//! Separate process from unit tests: winit permits only one event loop per process.
#![cfg(target_os = "windows")]

#[path = "../src/app/platform.rs"]
mod platform;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{
            GetWindowLongW, RegisterWindowMessageW, SendMessageW, SetWindowLongW, GWL_EXSTYLE,
            WS_EX_APPWINDOW, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
        },
    },
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalPosition,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    platform::windows::EventLoopBuilderExtWindows,
    window::WindowId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct TaskbarTest {
    result: Option<TestResult>,
}

impl ApplicationHandler for TaskbarTest {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.result = Some(check_window_transitions(event_loop));
        event_loop.exit();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

fn check_window_transitions(event_loop: &ActiveEventLoop) -> TestResult {
    // Exercise real show/hide without displaying a window on the user's desktop.
    let window = event_loop.create_window(
        platform::receiver_window_attributes()
            .with_title("Spout taskbar regression test")
            .with_position(PhysicalPosition::new(-32_000, -32_000))
            .with_active(false),
    )?;
    let hwnd = match window.window_handle()?.as_raw() {
        RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut _),
        _ => return Err("Expected a Windows window".into()),
    };
    let policy = platform::TaskbarPolicy::install(hwnd)?;
    check_style(hwnd)?;

    for visible in [true, false, true, false] {
        window.set_visible(visible);
        check_style(hwnd)?;
        if window.is_visible() != Some(visible) {
            return Err("Taskbar policy must not change requested window visibility".into());
        }
    }
    for decorated in [true, false] {
        window.set_decorations(decorated);
        check_style(hwnd)?;
    }

    // Also exercise a direct overwrite and the Explorer restart notification.
    let attempted = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    let attempted = (attempted | WS_EX_APPWINDOW.0) & !WS_EX_TOOLWINDOW.0;
    unsafe { SetWindowLongW(hwnd, GWL_EXSTYLE, attempted as i32) };
    check_style(hwnd)?;
    let explorer_restarted = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    if explorer_restarted == 0 {
        return Err(windows::core::Error::from_win32().into());
    }
    unsafe { SendMessageW(hwnd, explorer_restarted, WPARAM(0), LPARAM(0)) };
    check_style(hwnd)?;

    // The policy must detach before window destruction, leaving no callback owner.
    drop(policy);
    unsafe { SetWindowLongW(hwnd, GWL_EXSTYLE, attempted as i32) };
    if unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32 != attempted {
        return Err("Taskbar policy callback remained installed after drop".into());
    }
    Ok(())
}

fn check_style(hwnd: HWND) -> TestResult {
    let style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    if style & WS_EX_APPWINDOW.0 != 0
        || style & WS_EX_TOOLWINDOW.0 == 0
        || style & WS_EX_NOREDIRECTIONBITMAP.0 == 0
    {
        return Err(
            format!("Receiver taskbar/composition flags were overwritten: {style:#x}").into(),
        );
    }
    Ok(())
}

#[test]
#[ignore = "Requires the Windows desktop and event loop"]
fn receiver_stays_off_taskbar_across_window_state_changes() -> TestResult {
    let _com = platform::ComGuard::init()?;
    let event_loop = EventLoop::builder().with_any_thread(true).build()?;
    let mut application = TaskbarTest::default();
    event_loop.run_app(&mut application)?;
    application.result.ok_or("Window test did not run")?
}
