//! Rendering and alpha sampling share one immediate context on the UI thread.
#[path = "renderer/source_texture.rs"]
mod source_texture;
use crate::error::{AppError, ResultContext};
pub(crate) use source_texture::SenderResources;

use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::{IDXGISwapChain1, DXGI_PRESENT};

use crate::crop::{visible_bounds, AutoCrop, CropLayout, CropRect};
use crate::dx::keyed_mutex::KeyedMutexGuard;
use crate::dx::pipeline::{create_pipeline, Pipeline};
use crate::dx::staging::AlphaReadback;
use crate::dx::swapchain::create_swapchain;
use crate::frame_pacer::{FrameOutcome, FramePacer, FrameRate};
use crate::spout::{FrameCounter, SenderIdentity};

/// Complete resource set prepared before composition is changed. Installation
/// replaces it atomically; the renderer cannot hold a partial sender state.
pub struct PreparedSource {
    identity: SenderIdentity,
    sender: SenderResources,
    swapchain: IDXGISwapChain1,
    rtv: ID3D11RenderTargetView,
}

impl PreparedSource {
    pub fn swapchain(&self) -> &IDXGISwapChain1 {
        &self.swapchain
    }

    pub fn size(&self) -> (u32, u32) {
        (self.sender.width, self.sender.height)
    }
}

pub struct AlphaSample {
    pub alpha: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

const ALPHA_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, PartialEq, Eq)]
pub enum AlphaSetup {
    Unchanged,
    Created,
}

/// A completed read may update cropping without publishing a mouse mask, for
/// example when interaction is disabled or the sample belongs to an old epoch.
pub enum AlphaReadResult {
    NotReady,
    Completed { mask: Option<AlphaSample> },
}

struct AppliedViewport {
    crop: CropRect,
    layout: CropLayout,
}
impl AppliedViewport {
    fn new(crop: CropRect, scale: f32) -> Self {
        Self {
            crop,
            layout: CropLayout::new(crop, scale),
        }
    }
}

/// Owns all immediate-context access. UI/worker threads never share Draw/Map/Present.
pub struct Renderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    pipeline: Pipeline,
    installed: Option<PreparedSource>,
    readback: Option<AlphaReadback>,
    alpha_buf: Vec<u8>,
    pending_frame: bool,
    pacer: FramePacer,
    auto_crop: bool,
    crop: AutoCrop,
    viewport: AppliedViewport,
    next_alpha_extract: Instant,
    #[cfg(debug_assertions)]
    first_render_logged: bool,
}

impl Renderer {
    pub fn new(device: ID3D11Device, context: ID3D11DeviceContext) -> windows::core::Result<Self> {
        let pipeline = create_pipeline(&device)?;
        let now = Instant::now();
        let full = CropRect::full(1, 1);
        Ok(Self {
            device,
            context,
            pipeline,
            installed: None,
            readback: None,
            alpha_buf: Vec::new(),
            pending_frame: true,
            pacer: FramePacer::new(now),
            auto_crop: true,
            crop: AutoCrop::new(1, 1),
            viewport: AppliedViewport::new(full, 1.0),
            next_alpha_extract: now,
            #[cfg(debug_assertions)]
            first_render_logged: false,
        })
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }
    pub fn auto_crop_enabled(&self) -> bool {
        self.auto_crop
    }
    pub fn layout(&self) -> CropLayout {
        self.viewport.layout
    }
    pub fn viewport_changed(&self, crop: CropRect, scale: f32) -> bool {
        crop != self.viewport.crop || (scale - self.viewport.layout.scale).abs() > f32::EPSILON
    }
    /// Commit only after native composition and window updates succeed.
    pub fn commit_viewport(&mut self, crop: CropRect, scale: f32) -> CropLayout {
        self.viewport = AppliedViewport::new(crop, scale);
        self.viewport.layout
    }

    pub fn identity(&self) -> Option<&SenderIdentity> {
        self.installed.as_ref().map(|source| &source.identity)
    }
    pub fn swapchain(&self) -> Option<&IDXGISwapChain1> {
        self.installed.as_ref().map(PreparedSource::swapchain)
    }
    pub fn size(&self) -> (u32, u32) {
        self.installed.as_ref().map_or((1, 1), PreparedSource::size)
    }

    pub fn prepare(&self, identity: SenderIdentity) -> Result<PreparedSource, AppError> {
        let sender = SenderResources::open(&self.device, &identity)?;
        let (swapchain, rtv) = create_swapchain(&self.device, sender.width, sender.height)
            .context("Create sender swapchain")?;
        Ok(PreparedSource {
            identity,
            sender,
            swapchain,
            rtv,
        })
    }

    /// Call only after composition installation succeeds.
    pub fn install(&mut self, candidate: PreparedSource) {
        let (width, height) = candidate.size();
        #[cfg(debug_assertions)]
        crate::diagnostics::capture_sender_texture(&self.device, &self.context, &candidate.sender);
        self.installed = Some(candidate);
        self.readback = None;
        self.crop = AutoCrop::new(width, height);
        self.viewport = AppliedViewport::new(CropRect::full(width, height), 1.0);
        self.request_frame();
    }

    pub fn request_frame(&mut self) {
        self.pending_frame = true;
        self.pacer.request_frame();
        self.next_alpha_extract = Instant::now();
    }

    pub fn frame_due(&self, now: Instant) -> bool {
        self.pacer.is_due(now)
    }
    pub fn next_frame(
        &mut self,
        now: Instant,
        active: bool,
        outcome: FrameOutcome,
    ) -> Option<Instant> {
        self.pacer.next_wake(now, active, outcome)
    }
    pub fn rate(&self) -> FrameRate {
        self.pacer.rate()
    }
    pub fn set_rate(&mut self, rate: FrameRate) {
        self.pacer.set_rate(rate);
        self.request_frame();
    }
    pub fn cycle_rate(&mut self) -> FrameRate {
        let rate = self.pacer.cycle();
        self.request_frame();
        rate
    }

    /// Enable or disable stable auto-cropping. Reapplying the current value is a no-op.
    pub fn set_auto_crop(&mut self, enabled: bool) {
        if self.auto_crop == enabled {
            return;
        }
        self.auto_crop = enabled;
        let (w, h) = self.size();
        self.crop = AutoCrop::new(w, h);
        self.request_frame();
    }

    pub fn toggle_crop(&mut self) {
        self.set_auto_crop(!self.auto_crop);
    }

    pub fn target_crop(&mut self, now: Instant) -> CropRect {
        if self.auto_crop {
            self.crop.rect(now)
        } else {
            let (w, h) = self.size();
            CropRect::full(w, h)
        }
    }
    pub fn crop_deadline(&self) -> Option<Instant> {
        if self.auto_crop {
            self.crop.next_deadline()
        } else {
            None
        }
    }

    pub fn configure_alpha(&mut self, interaction: bool) -> windows::core::Result<AlphaSetup> {
        if !interaction && !self.auto_crop {
            self.readback = None;
            unsafe {
                self.context.OMSetRenderTargets(None, None);
                self.context.PSSetShaderResources(0, Some(&[None]));
            }
            self.alpha_buf = Vec::new();
            return Ok(AlphaSetup::Unchanged);
        }
        if self.readback.is_none() && self.installed.is_some() {
            let (w, h) = self.size();
            self.readback = Some(AlphaReadback::new(&self.device, w, h)?);
            self.request_frame();
            return Ok(AlphaSetup::Created);
        }
        Ok(AlphaSetup::Unchanged)
    }

    pub fn has_readback(&self) -> bool {
        self.readback
            .as_ref()
            .is_some_and(AlphaReadback::has_pending)
    }
    pub fn cancel_readback(&mut self) {
        if let Some(readback) = self.readback.as_mut() {
            readback.discard_pending();
        }
    }

    pub fn read_alpha(
        &mut self,
        interaction: bool,
        epoch: u64,
    ) -> windows::core::Result<AlphaReadResult> {
        let Some(readback) = self.readback.as_mut() else {
            return Ok(AlphaReadResult::NotReady);
        };
        let Some(sample_epoch) = readback.try_read(&self.context, &mut self.alpha_buf)? else {
            return Ok(AlphaReadResult::NotReady);
        };
        let (width, height) = self.size();
        if self.auto_crop {
            self.crop.observe(
                visible_bounds(&self.alpha_buf, width, height),
                Instant::now(),
            );
        }
        if interaction && epoch == sample_epoch {
            return Ok(AlphaReadResult::Completed {
                mask: Some(AlphaSample {
                    alpha: std::mem::take(&mut self.alpha_buf),
                    width,
                    height,
                }),
            });
        }
        Ok(AlphaReadResult::Completed { mask: None })
    }
    pub fn recycle_alpha(&mut self, buffer: Vec<u8>) {
        self.alpha_buf = buffer;
    }

    pub fn render(&mut self, epoch: u64, dragging: bool) -> windows::core::Result<FrameOutcome> {
        let Some(source) = self.installed.as_mut() else {
            return Ok(FrameOutcome::Busy);
        };
        let sender = &mut source.sender;
        let rtv = &source.rtv;
        let keyed = sender
            .keyed_mutex
            .as_ref()
            .and_then(KeyedMutexGuard::try_acquire);
        let named = if sender.keyed_mutex.is_none() {
            sender
                .named_mutex
                .as_ref()
                .and_then(|mutex| mutex.try_lock().ok().flatten())
        } else {
            None
        };
        if keyed.is_none() && named.is_none() {
            return Ok(FrameOutcome::Busy);
        }
        self.pending_frame |= sender
            .frame_counter
            .as_mut()
            .is_none_or(FrameCounter::is_new_frame);
        if !self.pending_frame {
            return Ok(FrameOutcome::SenderUnchanged);
        }
        let started = Instant::now();
        render_frame(
            &self.context,
            &self.pipeline,
            &sender.srv,
            rtv,
            sender.width,
            sender.height,
        );
        if !dragging && Instant::now() >= self.next_alpha_extract {
            if let Some(readback) = self.readback.as_mut() {
                if readback.enqueue_extract(&self.context, &self.pipeline, &sender.srv, epoch) {
                    self.next_alpha_extract = Instant::now() + ALPHA_SAMPLE_INTERVAL;
                }
            }
        }
        // Release CPU access guards before the presentation wait, preserving the
        // existing sharing protocol; all immediate-context work stays here.
        drop(keyed);
        drop(named);
        let swapchain = &source.swapchain;
        unsafe { swapchain.Present(1, DXGI_PRESENT(0)).ok()? };
        self.pending_frame = false;
        self.pacer.presented(started);
        #[cfg(debug_assertions)]
        if !self.first_render_logged {
            self.first_render_logged = true;
            log::info!("First frame presented");
            if crate::diagnostics::debug_capture_enabled() {
                if let Ok(buffer) = unsafe { swapchain.GetBuffer::<ID3D11Texture2D>(0) } {
                    if let Ok(exe) = std::env::current_exe() {
                        if let Some(dir) = exe.parent() {
                            crate::diagnostics::save_texture_to_bmp(
                                &self.device,
                                &self.context,
                                &buffer,
                                "BackBuffer",
                                &dir.join("debug_02_backbuffer_after_draw.bmp"),
                            );
                        }
                    }
                }
            }
        }
        Ok(FrameOutcome::Presented)
    }
}

/// Submit a full-source draw using an existing context and pipeline.
///
/// The caller owns synchronization, target dimensions and presentation. Do not
/// use the context concurrently with [`Renderer`] or another submission thread.
pub fn render_frame(
    context: &ID3D11DeviceContext,
    pipeline: &Pipeline,
    srv: &ID3D11ShaderResourceView,
    rtv: &ID3D11RenderTargetView,
    width: u32,
    height: u32,
) {
    unsafe {
        context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
        context.RSSetViewports(Some(&[D3D11_VIEWPORT {
            Width: width as f32,
            Height: height as f32,
            MaxDepth: 1.0,
            ..Default::default()
        }]));
        context.RSSetState(&pipeline.raster_state);
        context.VSSetShader(&pipeline.vs, None);
        context.PSSetShader(&pipeline.ps, None);
        context.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
        context.PSSetSamplers(0, Some(&[Some(pipeline.sampler.clone())]));
        context.IASetInputLayout(None);
        context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        context.Draw(3, 0);
    }
}

#[cfg(test)]
#[path = "renderer/tests.rs"]
mod tests;
