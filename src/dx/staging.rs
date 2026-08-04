use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

pub fn create_staging_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> windows::core::Result<ID3D11Texture2D> {
    if width == 0 || height == 0 {
        return Err(crate::dx::invalid_argument());
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        MipLevels: 1,
        ArraySize: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut tex = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex))? };
    tex.ok_or_else(crate::dx::missing_object)
}

pub fn read_alpha_from_staging(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    width: u32,
    height: u32,
    alpha: &mut Vec<u8>,
) {
    alpha.clear();
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    if unsafe { context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.is_err() {
        return;
    }
    let pitch = mapped.RowPitch;
    let ptr = mapped.pData as *const u8;
    let Some(pixel_count) = (width as usize).checked_mul(height as usize) else {
        unsafe { context.Unmap(staging, 0) };
        return;
    };
    alpha.reserve(pixel_count);
    for y in 0..height {
        let row_ptr = unsafe { ptr.add((y * pitch) as usize) };
        for x in 0..width {
            let bgra = unsafe { row_ptr.add((x * 4) as usize) };
            let a = unsafe { *bgra.offset(3) };
            alpha.push(a);
        }
    }
    unsafe { context.Unmap(staging, 0) };
}
