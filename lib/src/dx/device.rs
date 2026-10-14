use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_FLAG,
};

pub fn create_dx11_device_auto() -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    create_dx11_device(D3D_DRIVER_TYPE_HARDWARE)
        .or_else(|_| create_dx11_device(D3D_DRIVER_TYPE_WARP))
}

fn create_dx11_device(
    driver_type: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE,
) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    let mut feature_level = D3D_FEATURE_LEVEL::default();
    let feature_levels = [D3D_FEATURE_LEVEL_11_0];
    unsafe {
        D3D11CreateDevice(
            None,
            driver_type,
            None,
            D3D11_CREATE_DEVICE_FLAG(0),
            Some(&feature_levels),
            windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut context),
        )?;
    }
    Ok((
        device.ok_or_else(crate::dx::missing_object)?,
        context.ok_or_else(crate::dx::missing_object)?,
    ))
}
