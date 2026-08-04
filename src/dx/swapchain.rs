use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

pub fn create_swapchain(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> windows::core::Result<(IDXGISwapChain1, ID3D11RenderTargetView)> {
    if width == 0 || height == 0 {
        return Err(crate::dx::invalid_argument());
    }
    let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory1()? };
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: width,
        Height: height,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        ..Default::default()
    };
    let swapchain = unsafe { factory.CreateSwapChainForComposition(device, &desc, None)? };
    let bg = DXGI_RGBA {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    unsafe {
        swapchain.SetBackgroundColor(&bg).ok();
    }
    let backbuffer: ID3D11Texture2D = unsafe { swapchain.GetBuffer(0)? };
    let mut rtv = None;
    unsafe {
        device.CreateRenderTargetView(&backbuffer, None, Some(&mut rtv))?;
    }
    Ok((swapchain, rtv.ok_or_else(crate::dx::missing_object)?))
}
