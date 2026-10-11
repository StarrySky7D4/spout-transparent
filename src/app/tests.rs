use super::platform::ComGuard;
use super::runtime::window_should_be_visible;
use crate::crop::{CropLayout, CropRect};
use crate::dx::{
    device::create_dx11_device_auto, pipeline::create_pipeline, staging::AlphaReadback,
    swapchain::create_swapchain,
};
use crate::renderer::render_frame;
use std::time::{Duration, Instant};
use windows::{
    core::Interface,
    Win32::{
        Foundation::HWND, Graphics::Direct3D11::*,
        UI::WindowsAndMessaging::WS_EX_NOREDIRECTIONBITMAP,
    },
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
    let dcomp = crate::dx::composition::setup_dcomp(&device.cast()?, window.0)?;
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
        dcomp.apply_layout(window.0, position, previous, next)?;
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
#[ignore = "Requires a Windows D3D11 device; run with cargo test -- --ignored"]
fn full_screen_draw_and_alpha_readback_preserve_every_pixel() -> windows::core::Result<()> {
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

    let (device, context) = create_dx11_device_auto()?;
    let pipeline = create_pipeline(&device)?;
    // Odd dimensions also exercise padded staging rows and viewport edges.
    let (width, height) = (17u32, 9u32);
    let pixels: Vec<u8> = (0..width * height)
        .flat_map(|i| [17, 91, 203, [0, 64, 128, 255][i as usize % 4]])
        .collect();
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        ..Default::default()
    };
    let initial = D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr().cast(),
        SysMemPitch: width * 4,
        SysMemSlicePitch: 0,
    };
    let mut source = None;
    let mut srv = None;
    let mut target = None;
    let mut rtv = None;
    let mut staging = None;
    unsafe {
        device.CreateTexture2D(&desc, Some(&initial), Some(&mut source))?;
        device.CreateShaderResourceView(source.as_ref().unwrap(), None, Some(&mut srv))?;
        device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                ..desc
            },
            None,
            Some(&mut target),
        )?;
        device.CreateRenderTargetView(target.as_ref().unwrap(), None, Some(&mut rtv))?;
        device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..desc
            },
            None,
            Some(&mut staging),
        )?;
    }
    let srv = srv.unwrap();
    let rtv = rtv.unwrap();
    let staging = staging.unwrap();
    // Sentinel pixels expose holes when rendering without the production clear.
    unsafe { context.ClearRenderTargetView(&rtv, &[1.0, 0.0, 1.0, 1.0]) };
    render_frame(&context, &pipeline, &srv, &rtv, width, height);
    unsafe { context.CopyResource(&staging, target.as_ref().unwrap()) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))? };
    let mut actual = Vec::new();
    for y in 0..height as usize {
        let row = unsafe {
            std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                width as usize * 4,
            )
        };
        actual.extend_from_slice(row);
    }
    unsafe { context.Unmap(&staging, 0) };
    for (index, (input, output)) in pixels
        .chunks_exact(4)
        .zip(actual.chunks_exact(4))
        .enumerate()
    {
        for channel in 0..4 {
            let expected = if channel == 3 {
                input[3]
            } else {
                ((input[channel] as u32 * input[3] as u32 + 127) / 255) as u8
            };
            assert!(
                output[channel].abs_diff(expected) <= 1,
                "pixel {index}, channel {channel}"
            );
        }
    }

    let mut readback = AlphaReadback::new(&device, width, height)?;
    assert!(readback.enqueue_extract(&context, &pipeline, &srv, 7));
    assert!(readback.enqueue_extract(&context, &pipeline, &srv, 7));
    assert!(!readback.enqueue_extract(&context, &pipeline, &srv, 7));
    unsafe { context.Flush() };
    let mut alpha = Vec::new();
    for _ in 0..2 {
        let deadline = Instant::now() + Duration::from_secs(2);
        while readback.try_read(&context, &mut alpha)?.is_none() {
            assert!(Instant::now() < deadline, "Alpha readback timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(alpha.len(), (width * height) as usize);
        for (input, output) in pixels.chunks_exact(4).zip(&alpha) {
            assert!(input[3].abs_diff(*output) <= 1);
        }
    }
    assert!(readback.try_read(&context, &mut alpha)?.is_none());
    assert!(readback.enqueue_extract(&context, &pipeline, &srv, 7));
    Ok(())
}

#[test]
fn window_stays_hidden_without_a_sender() {
    assert!(!window_should_be_visible(false, true));
    assert!(window_should_be_visible(true, true));
    assert!(!window_should_be_visible(true, false));
}
