use super::platform::ComGuard;
use super::runtime::window_should_be_visible;
use spout_transparent::crop::{CropLayout, CropRect};
use spout_transparent::dx::{device::create_dx11_device_auto, swapchain::create_swapchain};
use windows::{
    core::Interface,
    Win32::{Foundation::HWND, UI::WindowsAndMessaging::WS_EX_NOREDIRECTIONBITMAP},
};

#[test]
#[ignore = "Requires a Windows D3D11 device and DirectComposition"]
fn composition_crop_restores_canvas_position_and_size() -> windows::core::Result<()> {
    use windows::core::w;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowRect, WS_POPUP,
    };
    let _com = ComGuard::init()?;
    struct TestWindow(HWND);
    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
    // This window stays hidden throughout the test.
    let window = TestWindow(unsafe {
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            w!("STATIC"),
            w!("Crop test"),
            WS_POPUP,
            -200,
            100,
            128,
            96,
            None,
            None,
            None,
            None,
        )?
    });
    let (device, _) = create_dx11_device_auto()?;
    let dcomp = spout_transparent::dx::composition::setup_dcomp(&device.cast()?, window.0)?;
    let (swapchain, _) = create_swapchain(&device, 128, 96)?;
    unsafe {
        dcomp.root_visual.SetContent(&swapchain)?;
        dcomp.device.Commit()?;
    }
    let full = CropRect::full(128, 96);
    let tight = CropRect {
        x: 31,
        y: 19,
        width: 40,
        height: 33,
    };
    let mut previous = CropLayout::new(full, 1.0);
    let mut position = winit::dpi::PhysicalPosition::new(-200, 100);
    for (rect, scale) in [(tight, 1.0), (tight, 1.05), (tight, 2.0), (full, 1.0)] {
        let next = CropLayout::new(rect, scale);
        dcomp.apply_layout(window.0, (position.x, position.y), previous, next)?;
        let mut actual = RECT::default();
        unsafe { GetWindowRect(window.0, &mut actual)? };
        assert_eq!(actual.left - next.offset_x, -200);
        assert_eq!(actual.top - next.offset_y, 100);
        assert_eq!(actual.right - actual.left, next.width as i32);
        assert_eq!(actual.bottom - actual.top, next.height as i32);
        position = winit::dpi::PhysicalPosition::new(actual.left, actual.top);
        previous = next;
    }
    Ok(())
}

#[test]
fn window_stays_hidden_without_a_sender() {
    assert!(!window_should_be_visible(false, true));
    assert!(window_should_be_visible(true, true));
    assert!(!window_should_be_visible(true, false));
}
