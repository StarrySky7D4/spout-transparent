use crate::crop::CropLayout;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionScaleTransform,
    IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Dxgi::IDXGISwapChain1;
use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

pub struct DCompResources {
    pub device: IDCompositionDevice,
    // Retain the HWND target for the entire visual tree lifetime.
    _target: IDCompositionTarget,
    pub root_visual: IDCompositionVisual,
    pub scale_transform: IDCompositionScaleTransform,
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
        _target: target,
        root_visual,
        scale_transform,
    })
}

impl DCompResources {
    /// Replace visual content and viewport together. A failed install keeps the
    /// old renderer resource set usable by restoring its visual content.
    pub fn install_content(
        &self,
        content: &IDXGISwapChain1,
        previous: Option<&IDXGISwapChain1>,
        hwnd: HWND,
        position: winit::dpi::PhysicalPosition<i32>,
        old: CropLayout,
        new: CropLayout,
    ) -> windows::core::Result<()> {
        let result = unsafe { self.root_visual.SetContent(content) }
            .and_then(|_| self.apply_layout(hwnd, position, old, new));
        if result.is_err() {
            unsafe {
                if let Some(previous) = previous {
                    let _ = self.root_visual.SetContent(previous);
                } else {
                    let _ = self
                        .root_visual
                        .SetContent(None::<&windows::core::IUnknown>);
                }
                let _ = self.device.Commit();
            }
        }
        result
    }

    pub fn apply_layout(
        &self,
        hwnd: HWND,
        position: winit::dpi::PhysicalPosition<i32>,
        old: CropLayout,
        new: CropLayout,
    ) -> windows::core::Result<()> {
        let scale_transform = &self.scale_transform;
        let root_visual = &self.root_visual;
        let dcomp_device = &self.device;
        let x = position
            .x
            .saturating_sub(old.offset_x)
            .saturating_add(new.offset_x);
        let y = position
            .y
            .saturating_sub(old.offset_y)
            .saturating_add(new.offset_y);
        let result = unsafe {
            // DComp applies the visual offset BEFORE its scale transform.
            scale_transform
                .SetScaleX2(new.scale)
                .and_then(|_| scale_transform.SetScaleY2(new.scale))
                .and_then(|_| root_visual.SetOffsetX2(-new.offset_x as f32 / new.scale))
                .and_then(|_| root_visual.SetOffsetY2(-new.offset_y as f32 / new.scale))
                .and_then(|_| dcomp_device.Commit())
                .and_then(|_| {
                    SetWindowPos(
                        hwnd,
                        None,
                        x,
                        y,
                        new.width as i32,
                        new.height as i32,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )
                })
        };
        if result.is_err() {
            unsafe {
                let _ = scale_transform.SetScaleX2(old.scale);
                let _ = scale_transform.SetScaleY2(old.scale);
                let _ = root_visual.SetOffsetX2(-old.offset_x as f32 / old.scale);
                let _ = root_visual.SetOffsetY2(-old.offset_y as f32 / old.scale);
                let _ = dcomp_device.Commit();
            }
        }
        result
    }
}
