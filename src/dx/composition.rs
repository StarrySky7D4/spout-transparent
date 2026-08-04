use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;

pub struct DCompResources {
    pub device: IDCompositionDevice,
    #[allow(dead_code)]
    pub target: IDCompositionTarget,
    pub root_visual: IDCompositionVisual,
}

pub fn setup_dcomp(dxgi_device: &IDXGIDevice, hwnd: HWND) -> windows::core::Result<DCompResources> {
    let device: IDCompositionDevice = unsafe { DCompositionCreateDevice(dxgi_device)? };
    log::info!("DCompositionCreateDevice OK");

    let target: IDCompositionTarget = unsafe { device.CreateTargetForHwnd(hwnd, true)? };
    log::info!("CreateTargetForHwnd(hwnd={hwnd:?}, topmost=true) OK");

    let root_visual: IDCompositionVisual = unsafe { device.CreateVisual()? };
    log::info!("CreateVisual OK");

    unsafe { target.SetRoot(&root_visual)? };
    log::info!("SetRoot OK");

    Ok(DCompResources {
        device,
        target,
        root_visual,
    })
}
