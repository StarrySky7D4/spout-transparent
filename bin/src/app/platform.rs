//! UI-thread native lifetimes and receiver window policy.
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{CoInitialize, CoUninitialize};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, STYLESTRUCT, WM_STYLECHANGING, WS_EX_APPWINDOW,
    WS_EX_TOOLWINDOW,
};
use winit::{
    dpi::PhysicalSize,
    platform::windows::WindowAttributesExtWindows,
    window::{Window, WindowAttributes},
};

pub(super) fn receiver_window_attributes() -> WindowAttributes {
    Window::default_attributes()
        .with_title("Spout Receiver (Transparent)")
        .with_inner_size(PhysicalSize::new(1, 1))
        .with_visible(false)
        .with_transparent(true)
        .with_decorations(false)
        .with_skip_taskbar(true)
        .with_no_redirection_bitmap(true)
}

const TASKBAR_POLICY_SUBCLASS_ID: usize = 3;

/// Keep native tool-window styles even when winit rewrites them on visibility
/// or decoration changes. The skip-taskbar attribute also handles Explorer restarts.
pub(super) struct TaskbarPolicy {
    hwnd: HWND,
}

impl TaskbarPolicy {
    pub(super) fn install(hwnd: HWND) -> windows::core::Result<Self> {
        let installed = unsafe {
            SetWindowSubclass(
                hwnd,
                Some(taskbar_policy_subclass),
                TASKBAR_POLICY_SUBCLASS_ID,
                0,
            )
        };
        if !installed.as_bool() {
            return Err(windows::core::Error::from_win32());
        }
        let policy = Self { hwnd };
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_EXSTYLE);
            SetWindowLongW(hwnd, GWL_EXSTYLE, tool_window_style(style as u32) as i32);
        }
        Ok(policy)
    }
}

impl Drop for TaskbarPolicy {
    fn drop(&mut self) {
        unsafe {
            let _ = RemoveWindowSubclass(
                self.hwnd,
                Some(taskbar_policy_subclass),
                TASKBAR_POLICY_SUBCLASS_ID,
            );
        }
    }
}

fn tool_window_style(style: u32) -> u32 {
    (style | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0
}

unsafe extern "system" fn taskbar_policy_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _ref_data: usize,
) -> LRESULT {
    if message == WM_STYLECHANGING && wparam.0 as i32 == GWL_EXSTYLE.0 {
        // WM_STYLECHANGING supplies a writable STYLESTRUCT for this call only.
        // Adjust the proposed flags rather than issuing a recursive style change.
        if let Some(style) = (lparam.0 as *mut STYLESTRUCT).as_mut() {
            style.styleNew = tool_window_style(style.styleNew);
        }
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

pub(super) struct ComGuard;
impl ComGuard {
    pub(super) fn init() -> windows::core::Result<Self> {
        unsafe {
            CoInitialize(None).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
