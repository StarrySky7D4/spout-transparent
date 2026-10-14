use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, GWL_EXSTYLE, WS_EX_LAYERED, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TRANSPARENT,
};
#[cfg(debug_assertions)]
pub(crate) fn log_window_style(hwnd: HWND) {
    let style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
    log::info!(
        "ExStyle=0x{style:X} LAYERED={} TRANSPARENT={} NOREDIR={}",
        style & WS_EX_LAYERED.0 as i32 != 0,
        style & WS_EX_TRANSPARENT.0 as i32 != 0,
        style & WS_EX_NOREDIRECTIONBITMAP.0 as i32 != 0
    );
}
