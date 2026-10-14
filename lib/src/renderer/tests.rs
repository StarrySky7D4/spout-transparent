use super::*;
use crate::spout::SenderName;
use windows::core::w;
use windows::core::Interface;
use windows::Win32::Graphics::Dxgi::IDXGIKeyedMutex;
use windows::Win32::Graphics::Dxgi::{Common::*, IDXGIResource};
use windows::Win32::System::Com::{CoInitialize, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
};

#[test]
#[ignore = "Requires Windows D3D11 shared textures and composition"]
fn busy_texture_preserves_pending_frame_and_old_alpha_epoch_is_rejected(
) -> windows::core::Result<()> {
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }
    unsafe {
        CoInitialize(None).ok()?;
    }
    let _com = Com;
    struct Window(windows::Win32::Foundation::HWND);
    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
    let window = Window(unsafe {
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            w!("STATIC"),
            w!("Renderer scheduling test"),
            WS_POPUP,
            0,
            0,
            16,
            16,
            None,
            None,
            None,
            None,
        )?
    });
    let name = format!("architecture-test-{}", std::process::id());
    let semaphore_name = std::ffi::CString::new(format!("{name}_Count_Semaphore")).unwrap();
    struct Handle(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
    let count = Handle(unsafe {
        windows_sys::Win32::System::Threading::CreateSemaphoreA(
            std::ptr::null(),
            5,
            i32::MAX,
            semaphore_name.as_ptr().cast(),
        )
    });
    assert!(!count.0.is_null());
    let (device, context) = crate::dx::device::create_dx11_device_auto()?;
    let mut renderer = Renderer::new(device.clone(), context.clone())?;
    let dcomp = crate::dx::composition::setup_dcomp(&device.cast()?, window.0)?;
    let desc = D3D11_TEXTURE2D_DESC {
        Width: 16,
        Height: 16,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
        ..Default::default()
    };
    let pixels = [255u8; 16 * 16 * 4];
    let initial = D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr().cast(),
        SysMemPitch: 16 * 4,
        SysMemSlicePitch: 0,
    };
    let mut texture = None;
    unsafe {
        device.CreateTexture2D(&desc, Some(&initial), Some(&mut texture))?;
    }
    let texture = texture.unwrap();
    let resource: IDXGIResource = texture.cast()?;
    let handle = unsafe { resource.GetSharedHandle()? };
    let identity = SenderIdentity {
        name: SenderName::from_bytes(&name).unwrap(),
        share_handle: handle.0 as usize as u32,
        generation: 1,
    };
    let candidate = renderer
        .prepare(identity)
        .expect("shared texture candidate");
    unsafe {
        dcomp.root_visual.SetContent(&candidate.swapchain)?;
        dcomp.device.Commit()?;
    }
    renderer.install(candidate);
    renderer.configure_alpha(true)?;
    assert_eq!(renderer.render(7, false)?, FrameOutcome::Presented);
    unsafe {
        context.Flush();
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let result = renderer.read_alpha(true, 8)?;
        if let AlphaReadResult::Completed { mask } = result {
            assert!(
                mask.is_none(),
                "the previous interaction session must not publish a mask"
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(renderer.render(8, false)?, FrameOutcome::SenderUnchanged);
    renderer.request_frame();
    let mutex: IDXGIKeyedMutex = texture.cast()?;
    unsafe {
        mutex.AcquireSync(0, 0)?;
        mutex.ReleaseSync(7)?;
    }
    assert_eq!(renderer.render(8, false)?, FrameOutcome::Busy);
    assert!(renderer.pending_frame);
    unsafe {
        mutex.AcquireSync(7, 0)?;
        mutex.ReleaseSync(0)?;
    }
    assert_eq!(renderer.render(8, false)?, FrameOutcome::Presented);
    assert!(!renderer.pending_frame);
    unsafe {
        context.Flush();
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let result = renderer.read_alpha(true, 8)?;
        if let AlphaReadResult::Completed { mask } = result {
            let sample = mask.expect("the current session receives its alpha mask");
            assert_eq!((sample.width, sample.height), (16, 16));
            assert!(sample.alpha.iter().all(|alpha| *alpha == 255));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[test]
#[ignore = "Requires a Windows D3D11 device; run with cargo test -- --ignored"]
fn full_screen_draw_and_alpha_readback_preserve_every_pixel() -> windows::core::Result<()> {
    use crate::dx::{
        device::create_dx11_device_auto, pipeline::create_pipeline, staging::AlphaReadback,
    };
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
