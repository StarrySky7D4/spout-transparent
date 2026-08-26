use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionScaleTransform,
    IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISwapChain1};

pub struct DCompResources {
    pub device: IDCompositionDevice,
    pub target: IDCompositionTarget,
    pub root_visual: IDCompositionVisual,
    pub scale_transform: IDCompositionScaleTransform,
}

impl DCompResources {
    pub fn is_valid(&self) -> windows::core::Result<bool> {
        unsafe { self.device.CheckDeviceState().map(|valid| valid.as_bool()) }
    }

    pub fn attach_swapchain(
        &self,
        swapchain: &IDXGISwapChain1,
        scale: f32,
    ) -> windows::core::Result<()> {
        unsafe {
            self.scale_transform.SetScaleX2(scale)?;
            self.scale_transform.SetScaleY2(scale)?;
            self.root_visual.SetContent(swapchain)?;
            self.device.Commit()
        }
    }

    pub fn set_scale(&self, scale: f32) -> windows::core::Result<()> {
        unsafe {
            self.scale_transform.SetScaleX2(scale)?;
            self.scale_transform.SetScaleY2(scale)?;
            self.device.Commit()
        }
    }

    pub fn detach(&self) {
        unsafe {
            let _ = self.target.SetRoot(None::<&IDCompositionVisual>);
            let _ = self.device.Commit();
        }
    }
}

pub fn setup_dcomp(dxgi_device: &IDXGIDevice, hwnd: HWND) -> windows::core::Result<DCompResources> {
    let device: IDCompositionDevice = unsafe { DCompositionCreateDevice(dxgi_device)? };
    log::info!("DCompositionCreateDevice OK");

    let target: IDCompositionTarget = unsafe { device.CreateTargetForHwnd(hwnd, true)? };
    log::info!("CreateTargetForHwnd(hwnd={hwnd:?}, topmost=true) OK");

    let root_visual: IDCompositionVisual = unsafe { device.CreateVisual()? };
    log::info!("CreateVisual OK");

    let scale_transform = unsafe { device.CreateScaleTransform()? };
    unsafe { root_visual.SetTransform(&scale_transform)? };

    unsafe { target.SetRoot(&root_visual)? };
    log::info!("SetRoot OK");

    Ok(DCompResources {
        device,
        target,
        root_visual,
        scale_transform,
    })
}
