use crate::{
    boundary::{self, api, insert, with_registry, Error, Resource},
    receiver::delay_ms,
    types::*,
    wrong_resource,
};
use spout_transparent::{
    dx::{
        composition::{setup_dcomp, DCompResources},
        device::create_dx11_device_auto,
    },
    windows::{
        core::Interface,
        Win32::{
            Foundation::{HWND, RECT},
            UI::WindowsAndMessaging::{GetWindowRect, GetWindowThreadProcessId, IsWindow},
        },
    },
    AlphaReadResult, AlphaSample, CropLayout, CropRect, FrameOutcome, FrameRate, Renderer,
};
use std::{ffi::c_void, time::Instant};

pub struct RenderState {
    // Composition releases its reference to the swapchain before Renderer drops.
    composition: DCompResources,
    renderer: Renderer,
    hwnd: HWND,
    next_frame: Instant,
    crop: CropRect,
    alpha_enabled: bool,
    alpha_setup_needed: bool,
    epoch: u64,
    sample: Option<AlphaSample>,
}
impl RenderState {
    fn discard_sample(&mut self) {
        if let Some(sample) = self.sample.take() {
            self.renderer.recycle_alpha(sample.alpha);
        }
        self.renderer.cancel_readback();
        self.epoch = self.epoch.wrapping_add(1);
    }
    fn position(&self) -> Result<(i32, i32), Error> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut rect) }.map_err(Error::gpu)?;
        Ok((rect.left, rect.top))
    }
}

/// Create a renderer for an existing HWND on its owning OS thread.
/// The host must initialize COM first and keep both COM and HWND alive until release.
/// # Safety
/// Hwnd is a valid host-owned HWND; out is writable aligned u64 storage.
#[no_mangle]
pub unsafe extern "C" fn spt_renderer_create(hwnd: *mut c_void, out: *mut u64) -> i32 {
    api(|| {
        unsafe { boundary::output(out, 0)? };
        let hwnd = HWND(hwnd);
        let mut process_id = 0;
        let owner_thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
        if !unsafe { IsWindow(hwnd) }.as_bool()
            || process_id != std::process::id()
            || owner_thread
                != unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
        {
            return Err(Error::argument(
                "HWND must belong to this process and calling thread",
            ));
        }
        let (device, context) = create_dx11_device_auto().map_err(Error::gpu)?;
        let composition =
            setup_dcomp(&device.cast().map_err(Error::gpu)?, hwnd).map_err(Error::gpu)?;
        let mut renderer = Renderer::new(device, context).map_err(Error::gpu)?;
        // C callers opt in to alpha and drive crop/layout themselves.
        renderer.set_auto_crop(false);
        let state = RenderState {
            composition,
            renderer,
            hwnd,
            next_frame: Instant::now(),
            crop: CropRect::full(1, 1),
            alpha_enabled: false,
            alpha_setup_needed: false,
            epoch: 0,
            sample: None,
        };
        let handle = insert(Resource::Renderer(Box::new(state)))?;
        unsafe { boundary::output(out, handle) }
    })
}

/// Prepare and install the receiver's current resource. Source changes require reattach.
#[no_mangle]
pub extern "C" fn spt_renderer_attach(renderer: u64, receiver: u64) -> i32 {
    api(|| {
        with_registry(|registry| {
            if renderer == receiver {
                return Err(Error::handle());
            }
            let [renderer, receiver] = registry.get_disjoint_mut([&renderer, &receiver]);
            let (Some(Resource::Renderer(state)), Some(Resource::Receiver(receiver))) =
                (renderer, receiver)
            else {
                return wrong_resource();
            };
            let identity = receiver
                .identity()
                .ok_or_else(|| Error::not_found("Receiver disconnected"))?;
            let candidate = state.renderer.prepare(identity).map_err(Error::gpu)?;
            let (width, height) = candidate.size();
            let crop = CropRect::full(width, height);
            state
                .composition
                .install_content(
                    candidate.swapchain(),
                    state.renderer.swapchain(),
                    state.hwnd,
                    state.position()?,
                    state.renderer.layout(),
                    CropLayout::new(crop, 1.0),
                )
                .map_err(Error::gpu)?;
            state.discard_sample();
            state.renderer.install(candidate);
            state.crop = crop;
            state.alpha_setup_needed = state.alpha_enabled;
            state.next_frame = Instant::now();
            Ok(())
        })
    })
}

/// Draw/Present only while the receiver still matches the installed generation.
/// Outcomes: 1=presented, 2=unchanged, 3=lock busy, 4=frame deadline not due.
/// # Safety
/// Outcome points to writable aligned u32 storage.
#[no_mangle]
pub unsafe extern "C" fn spt_renderer_render(
    renderer: u64,
    receiver: u64,
    outcome: *mut u32,
) -> i32 {
    api(|| {
        boundary::pointer(outcome)?;
        with_registry(|registry| {
            if renderer == receiver {
                return Err(Error::handle());
            }
            let [renderer, receiver] = registry.get_disjoint_mut([&renderer, &receiver]);
            let (Some(Resource::Renderer(state)), Some(Resource::Receiver(receiver))) =
                (renderer, receiver)
            else {
                return wrong_resource();
            };
            if !state
                .renderer
                .identity()
                .is_some_and(|identity| receiver.matches(identity))
            {
                return Err(Error::not_found(
                    "Receiver is disconnected or renderer needs reattach",
                ));
            }
            if state.alpha_setup_needed {
                state
                    .renderer
                    .configure_alpha(state.alpha_enabled)
                    .map_err(Error::gpu)?;
                state.alpha_setup_needed = false;
            }
            let now = Instant::now();
            if now < state.next_frame {
                return unsafe { boundary::output(outcome, 4) };
            }
            let result = state
                .renderer
                .render(state.epoch, false)
                .map_err(Error::gpu)?;
            state.next_frame = state
                .renderer
                .next_frame(Instant::now(), true, result)
                .unwrap_or_else(Instant::now);
            unsafe {
                boundary::output(
                    outcome,
                    match result {
                        FrameOutcome::Presented => 1,
                        FrameOutcome::SenderUnchanged => 2,
                        FrameOutcome::Busy => 3,
                    },
                )
            }
        })
    })
}

/// Milliseconds to next frame attempt, rounded up. Present may itself block for vsync.
/// # Safety
/// Delay points to writable aligned u64 storage.
#[no_mangle]
pub unsafe extern "C" fn spt_renderer_next_frame_ms(handle: u64, delay: *mut u64) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get(&handle) {
            Some(Resource::Renderer(state)) => unsafe {
                boundary::output(delay, delay_ms(state.next_frame))
            },
            _ => wrong_resource(),
        })
    })
}

/// Supported rates are 0 (unlimited), 30, 60 and 120.
#[no_mangle]
pub extern "C" fn spt_renderer_set_frame_rate(handle: u64, fps: u32) -> i32 {
    api(|| {
        let rate = match fps {
            0 => FrameRate::Unlimited,
            30 => FrameRate::Fps30,
            60 => FrameRate::Fps60,
            120 => FrameRate::Fps120,
            _ => return Err(Error::argument("Frame rate must be 0, 30, 60 or 120")),
        };
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Renderer(state)) => {
                state.renderer.set_rate(rate);
                state.next_frame = Instant::now();
                Ok(())
            }
            _ => wrong_resource(),
        })
    })
}

/// Request a frame even if the sender's counter has not changed.
#[no_mangle]
pub extern "C" fn spt_renderer_request_frame(handle: u64) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Renderer(state)) => {
                state.renderer.request_frame();
                state.next_frame = Instant::now();
                Ok(())
            }
            _ => wrong_resource(),
        })
    })
}

/// Enable (1) or disable (0) alpha extraction. Disabling releases readback buffers.
#[no_mangle]
pub extern "C" fn spt_renderer_set_alpha(handle: u64, enabled: u32) -> i32 {
    api(|| {
        if enabled > 1 {
            return Err(Error::argument("Boolean values must be 0 or 1"));
        }
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Renderer(state)) => {
                let enabled = enabled != 0;
                if enabled != state.alpha_enabled {
                    state.discard_sample();
                }
                state
                    .renderer
                    .configure_alpha(enabled)
                    .map_err(Error::gpu)?;
                state.alpha_enabled = enabled;
                state.alpha_setup_needed = enabled && state.renderer.identity().is_none();
                state.renderer.request_frame();
                state.next_frame = Instant::now();
                Ok(())
            }
            _ => wrong_resource(),
        })
    })
}

/// Whether an alpha slot is pending or a completed sample is latched.
/// Check after rendering; schedule short readback retries only while pending.
/// # Safety
/// Pending points to writable aligned u32 storage.
#[no_mangle]
pub unsafe extern "C" fn spt_renderer_has_readback(handle: u64, pending: *mut u32) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get(&handle) {
            Some(Resource::Renderer(state)) => unsafe {
                boundary::output(
                    pending,
                    u32::from(state.sample.is_some() || state.renderer.has_readback()),
                )
            },
            _ => wrong_resource(),
        })
    })
}

/// Copy one source-resolution R8 sample, latched across a buffer-size probe.
/// # Safety
/// Outputs are writable aligned storage; non-null buffer covers capacity writable bytes.
#[no_mangle]
pub unsafe extern "C" fn spt_renderer_read_alpha(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
    width: *mut u32,
    height: *mut u32,
) -> i32 {
    api(|| {
        boundary::pointer(required)?;
        boundary::pointer(width)?;
        boundary::pointer(height)?;
        if capacity > isize::MAX as usize || (buffer.is_null() && capacity != 0) {
            return Err(Error::argument("Invalid alpha output buffer"));
        }
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Renderer(state)) => {
                if !state.alpha_enabled {
                    return Err(Error::argument("Alpha readback disabled"));
                }
                if state.sample.is_none() {
                    if let AlphaReadResult::Completed { mask } = state
                        .renderer
                        .read_alpha(true, state.epoch)
                        .map_err(Error::gpu)?
                    {
                        state.sample = mask;
                    }
                }
                let sample = state.sample.as_ref().ok_or_else(Error::pending)?;
                unsafe {
                    boundary::output(width, sample.width)?;
                    boundary::output(height, sample.height)?;
                    boundary::copy_bytes(&sample.alpha, buffer, capacity, required, false)?;
                }
                if let Some(sample) = state.sample.take() {
                    state.renderer.recycle_alpha(sample.alpha);
                }
                Ok(())
            }
            _ => wrong_resource(),
        })
    })
}

/// Apply a source-space crop and scale to the supplied HWND; preserves canvas anchor.
/// # Safety
/// Crop points to readable aligned SptRect storage.
#[no_mangle]
pub unsafe extern "C" fn spt_renderer_set_layout(
    handle: u64,
    crop: *const SptRect,
    scale: f32,
) -> i32 {
    api(|| {
        boundary::pointer(crop)?;
        if !scale.is_finite() || !(0.1..=5.0).contains(&scale) {
            return Err(Error::argument(
                "Scale must be finite and between 0.1 and 5",
            ));
        }
        let crop: CropRect = unsafe { crop.read() }.into();
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Renderer(state)) => {
                if state.renderer.identity().is_none() {
                    return Err(Error::not_found("Renderer has no source"));
                }
                let (width, height) = state.renderer.size();
                if crop.width == 0
                    || crop.height == 0
                    || crop
                        .x
                        .checked_add(crop.width)
                        .is_none_or(|right| right > width)
                    || crop
                        .y
                        .checked_add(crop.height)
                        .is_none_or(|bottom| bottom > height)
                    || width as f32 * scale > 16_384.0
                    || height as f32 * scale > 16_384.0
                {
                    return Err(Error::argument("Crop or scaled source exceeds its bounds"));
                }
                if state.renderer.viewport_changed(crop, scale) {
                    state
                        .composition
                        .apply_layout(
                            state.hwnd,
                            state.position()?,
                            state.renderer.layout(),
                            CropLayout::new(crop, scale),
                        )
                        .map_err(Error::gpu)?;
                    state.renderer.commit_viewport(crop, scale);
                    state.crop = crop;
                }
                Ok(())
            }
            _ => wrong_resource(),
        })
    })
}

/// Read the committed crop, scale, quantized offsets and physical window dimensions.
/// # Safety
/// Layout points to writable aligned SptLayout storage.
#[no_mangle]
pub unsafe extern "C" fn spt_renderer_layout(handle: u64, layout: *mut SptLayout) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get(&handle) {
            Some(Resource::Renderer(state)) => unsafe {
                boundary::output(layout, SptLayout::new(state.crop, state.renderer.layout()))
            },
            _ => wrong_resource(),
        })
    })
}
