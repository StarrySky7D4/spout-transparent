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
        name: SenderName::for_test(&name),
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
