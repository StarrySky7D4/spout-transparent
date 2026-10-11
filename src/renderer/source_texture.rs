//! One opened sender texture and the synchronization mechanisms it supports.
use crate::{
    error::{AppError, ResultContext},
    spout::{FrameCounter, NamedMutex, SenderIdentity},
};
use windows::core::Interface;
use windows::Win32::{
    Foundation::HANDLE,
    Graphics::{Direct3D::D3D10_1_SRV_DIMENSION_TEXTURE2D, Direct3D11::*, Dxgi::IDXGIKeyedMutex},
};

pub(crate) struct SenderResources {
    #[cfg(debug_assertions)]
    pub(crate) tex: ID3D11Texture2D,
    pub(super) srv: ID3D11ShaderResourceView,
    pub(super) keyed_mutex: Option<IDXGIKeyedMutex>,
    pub(super) named_mutex: Option<NamedMutex>,
    pub(super) frame_counter: Option<FrameCounter>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl SenderResources {
    pub(super) fn open(device: &ID3D11Device, identity: &SenderIdentity) -> Result<Self, AppError> {
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe { device.OpenSharedResource(HANDLE(identity.raw_handle()), &mut tex) }
            .context("Open shared sender texture")?;
        let tex = tex.ok_or_else(|| AppError::message("OpenSharedResource returned no texture"))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut desc) };
        let mut srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: desc.Format,
            ViewDimension: D3D10_1_SRV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        srv_desc.Anonymous.Texture2D.MipLevels = 1;
        let mut srv = None;
        unsafe { device.CreateShaderResourceView(&tex, Some(&srv_desc), Some(&mut srv)) }
            .context("Create sender shader resource view")?;
        let keyed_mutex = tex.cast::<IDXGIKeyedMutex>().ok();
        let named_mutex = if keyed_mutex.is_none() {
            Some(
                NamedMutex::for_sender_texture(&identity.name)
                    .context("Open sender access mutex")?,
            )
        } else {
            None
        };
        let frame_counter = FrameCounter::for_sender(&identity.name)
            .map_err(|error| {
                log::warn!("Frame counter unavailable: {error}");
                error
            })
            .ok();
        log::info!(
            "Sender texture: {}x{}, format {}, keyed mutex {}",
            desc.Width,
            desc.Height,
            desc.Format.0,
            keyed_mutex.is_some()
        );
        Ok(Self {
            #[cfg(debug_assertions)]
            tex,
            srv: srv.ok_or_else(|| AppError::message("No shader resource view"))?,
            keyed_mutex,
            named_mutex,
            frame_counter,
            width: desc.Width,
            height: desc.Height,
        })
    }
}
