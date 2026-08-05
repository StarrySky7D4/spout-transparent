use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use windows::core::{w, Error, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass, Shell_NotifyIconW, NIF_ICON,
    NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetWindowLongW, LoadIconW,
    PostMessageW, RegisterWindowMessageW, SetForegroundWindow, SetWindowLongW, TrackPopupMenu,
    GWL_EXSTYLE, HMENU, IDI_APPLICATION, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    MF_UNCHECKED, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU,
    WM_LBUTTONDBLCLK, WM_NULL, WM_RBUTTONUP, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

use crate::config::FrameRate;

const TRAY_ICON_ID: u32 = 1;
const TRAY_SUBCLASS_ID: usize = 2;
const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;

const CMD_TOGGLE_VISIBILITY: u32 = 1001;
const CMD_TOGGLE_INTERACTION: u32 = 1002;
const CMD_TOGGLE_TOPMOST: u32 = 1003;
const CMD_FPS_UNLIMITED: u32 = 1010;
const CMD_FPS_120: u32 = 1011;
const CMD_FPS_60: u32 = 1012;
const CMD_FPS_30: u32 = 1013;
const CMD_QUIT: u32 = 1099;
const CMD_SOURCE_BASE: u32 = 2000;
const MAX_SOURCE_COMMANDS: usize = 512;

const STATE_VISIBLE: u32 = 1 << 0;
const STATE_INTERACTION: u32 = 1 << 1;
const STATE_TOPMOST: u32 = 1 << 2;
const STATE_RATE_SHIFT: u32 = 3;
const STATE_RATE_MASK: u32 = 0b11 << STATE_RATE_SHIFT;
const STATE_HAS_SOURCE: u32 = 1 << 5;

static PENDING_COMMAND: AtomicU32 = AtomicU32::new(0);
static MENU_STATE: AtomicU32 = AtomicU32::new(0);
static TASKBAR_CREATED_MESSAGE: AtomicU32 = AtomicU32::new(0);
static MENU_OPEN: AtomicBool = AtomicBool::new(false);
static SOURCE_MENU: Mutex<SourceMenuState> = Mutex::new(SourceMenuState {
    names: Vec::new(),
    selected: None,
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayAction {
    ToggleVisibility,
    ToggleInteraction,
    ToggleTopmost,
    SetFrameRate(FrameRate),
    SelectSource(usize),
    Quit,
}

#[derive(Clone, Copy, Debug)]
pub struct TrayState {
    pub visible: bool,
    pub interaction: bool,
    pub topmost: bool,
    pub frame_rate: FrameRate,
    pub has_source: bool,
}

struct SourceMenuState {
    names: Vec<String>,
    selected: Option<String>,
}

pub struct TrayIcon {
    hwnd: HWND,
}

impl TrayIcon {
    pub fn install(hwnd: HWND) -> Result<Self> {
        hide_from_taskbar(hwnd);

        let subclass_ok =
            unsafe { SetWindowSubclass(hwnd, Some(tray_subclass), TRAY_SUBCLASS_ID, 0).as_bool() };
        if !subclass_ok {
            return Err(Error::from_win32());
        }

        TASKBAR_CREATED_MESSAGE.store(
            unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            Ordering::Release,
        );

        if let Err(error) = add_icon(hwnd) {
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(tray_subclass), TRAY_SUBCLASS_ID);
            }
            return Err(error);
        }

        Ok(Self { hwnd })
    }

    pub fn update_state(&self, state: TrayState) {
        MENU_STATE.store(encode_state(state), Ordering::Release);
    }

    pub fn update_sources(&self, names: Vec<String>, selected: Option<String>) {
        if let Ok(mut menu) = SOURCE_MENU.lock() {
            menu.names = names;
            menu.selected = selected;
        }
    }

    pub fn take_action(&self) -> Option<TrayAction> {
        action_from_command(PENDING_COMMAND.swap(0, Ordering::AcqRel))
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let data = icon_data(self.hwnd, None);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            let _ = RemoveWindowSubclass(self.hwnd, Some(tray_subclass), TRAY_SUBCLASS_ID);
        }
        PENDING_COMMAND.store(0, Ordering::Release);
    }
}

fn hide_from_taskbar(hwnd: HWND) {
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        SetWindowLongW(
            hwnd,
            GWL_EXSTYLE,
            (style | WS_EX_TOOLWINDOW.0 as i32) & !(WS_EX_APPWINDOW.0 as i32),
        );
    }
}

fn add_icon(hwnd: HWND) -> Result<()> {
    let icon = unsafe { LoadIconW(None, IDI_APPLICATION)? };
    let data = icon_data(hwnd, Some(icon));
    if unsafe { Shell_NotifyIconW(NIM_ADD, &data).as_bool() } {
        Ok(())
    } else {
        Err(Error::from_win32())
    }
}

fn icon_data(
    hwnd: HWND,
    icon: Option<windows::Win32::UI::WindowsAndMessaging::HICON>,
) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        ..Default::default()
    };
    if let Some(icon) = icon {
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
        data.hIcon = icon;
        for (target, source) in data
            .szTip
            .iter_mut()
            .zip("Spout Transparent".encode_utf16())
        {
            *target = source;
        }
    }
    data
}

unsafe extern "system" fn tray_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _ref_data: usize,
) -> LRESULT {
    if message == TRAY_CALLBACK_MESSAGE {
        match lparam.0 as u32 {
            WM_RBUTTONUP | WM_CONTEXTMENU => {
                show_context_menu_async(hwnd);
                return LRESULT(0);
            }
            WM_LBUTTONDBLCLK => {
                PENDING_COMMAND.store(CMD_TOGGLE_VISIBILITY, Ordering::Release);
                return LRESULT(0);
            }
            _ => {}
        }
    }

    let taskbar_created = TASKBAR_CREATED_MESSAGE.load(Ordering::Acquire);
    if taskbar_created != 0 && message == taskbar_created {
        if let Err(error) = add_icon(hwnd) {
            log::error!("Failed to restore tray icon: {error:?}");
        }
        return LRESULT(0);
    }

    DefSubclassProc(hwnd, message, wparam, lparam)
}

fn show_context_menu_async(hwnd: HWND) {
    if MENU_OPEN
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    // TrackPopupMenu blocks until the menu closes. Run it on a dedicated thread
    // so the winit event loop can keep polling Spout and presenting frames.
    let hwnd_value = hwnd.0 as isize;
    let spawn_result = std::thread::Builder::new()
        .name("spout-tray-menu".to_string())
        .spawn(move || {
            let _guard = MenuOpenGuard;
            let hwnd = HWND(hwnd_value as *mut std::ffi::c_void);
            if let Err(error) = show_context_menu(hwnd) {
                log::error!("Tray menu failed: {error:?}");
            }
            unsafe {
                let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
            }
        });

    if let Err(error) = spawn_result {
        MENU_OPEN.store(false, Ordering::Release);
        log::error!("Failed to start tray menu thread: {error}");
    }
}

struct MenuOpenGuard;

impl Drop for MenuOpenGuard {
    fn drop(&mut self) {
        MENU_OPEN.store(false, Ordering::Release);
    }
}

fn show_context_menu(hwnd: HWND) -> Result<()> {
    let state = decode_state(MENU_STATE.load(Ordering::Acquire));
    let menu = OwnedMenu::new()?;
    let source_menu = OwnedMenu::new()?;
    let frame_menu = OwnedMenu::new()?;

    unsafe {
        AppendMenuW(
            menu.handle(),
            MF_STRING | disabled(!state.has_source),
            CMD_TOGGLE_VISIBILITY as usize,
            if !state.has_source {
                w!("无可用来源")
            } else if state.visible {
                w!("隐藏窗口")
            } else {
                w!("显示窗口")
            },
        )?;

        if let Ok(sources) = SOURCE_MENU.lock() {
            if sources.names.is_empty() {
                AppendMenuW(
                    source_menu.handle(),
                    MF_STRING | MF_GRAYED,
                    0,
                    w!("无可用来源"),
                )?;
            } else {
                for (index, name) in sources.names.iter().take(MAX_SOURCE_COMMANDS).enumerate() {
                    let menu_name = name.replace('&', "&&");
                    let wide_name: Vec<u16> =
                        menu_name.encode_utf16().chain(std::iter::once(0)).collect();
                    AppendMenuW(
                        source_menu.handle(),
                        MF_STRING | checked(sources.selected.as_ref() == Some(name)),
                        (CMD_SOURCE_BASE + index as u32) as usize,
                        PCWSTR(wide_name.as_ptr()),
                    )?;
                }
            }
        }
        AppendMenuW(
            menu.handle(),
            MF_POPUP,
            source_menu.into_submenu(),
            w!("来源"),
        )?;
        AppendMenuW(
            menu.handle(),
            MF_STRING | checked(state.interaction),
            CMD_TOGGLE_INTERACTION as usize,
            w!("交互模式"),
        )?;
        AppendMenuW(
            menu.handle(),
            MF_STRING | checked(state.topmost),
            CMD_TOGGLE_TOPMOST as usize,
            w!("窗口置顶"),
        )?;
        AppendMenuW(
            frame_menu.handle(),
            MF_STRING | checked(state.frame_rate == FrameRate::Unlimited),
            CMD_FPS_UNLIMITED as usize,
            w!("不限制"),
        )?;
        AppendMenuW(
            frame_menu.handle(),
            MF_STRING | checked(state.frame_rate == FrameRate::Fps120),
            CMD_FPS_120 as usize,
            w!("120 FPS"),
        )?;
        AppendMenuW(
            frame_menu.handle(),
            MF_STRING | checked(state.frame_rate == FrameRate::Fps60),
            CMD_FPS_60 as usize,
            w!("60 FPS"),
        )?;
        AppendMenuW(
            frame_menu.handle(),
            MF_STRING | checked(state.frame_rate == FrameRate::Fps30),
            CMD_FPS_30 as usize,
            w!("30 FPS"),
        )?;
        AppendMenuW(
            menu.handle(),
            MF_POPUP,
            frame_menu.into_submenu(),
            w!("帧率"),
        )?;
        AppendMenuW(menu.handle(), MF_SEPARATOR, 0, w!(""))?;
        AppendMenuW(menu.handle(), MF_STRING, CMD_QUIT as usize, w!("退出"))?;

        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor)?;
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu.handle(),
            TPM_NONOTIFY | TPM_RETURNCMD | TPM_RIGHTBUTTON,
            cursor.x,
            cursor.y,
            0,
            hwnd,
            None,
        )
        .0 as u32;
        if command != 0 {
            PENDING_COMMAND.store(command, Ordering::Release);
        }
    }

    Ok(())
}

fn checked(value: bool) -> windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS {
    if value {
        MF_CHECKED
    } else {
        MF_UNCHECKED
    }
}

fn disabled(value: bool) -> windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS {
    if value {
        MF_GRAYED
    } else {
        MF_UNCHECKED
    }
}

fn encode_state(state: TrayState) -> u32 {
    let mut encoded = 0;
    if state.visible {
        encoded |= STATE_VISIBLE;
    }
    if state.interaction {
        encoded |= STATE_INTERACTION;
    }
    if state.topmost {
        encoded |= STATE_TOPMOST;
    }
    if state.has_source {
        encoded |= STATE_HAS_SOURCE;
    }
    encoded | (encode_rate(state.frame_rate) << STATE_RATE_SHIFT)
}

fn decode_state(encoded: u32) -> TrayState {
    TrayState {
        visible: encoded & STATE_VISIBLE != 0,
        interaction: encoded & STATE_INTERACTION != 0,
        topmost: encoded & STATE_TOPMOST != 0,
        frame_rate: decode_rate((encoded & STATE_RATE_MASK) >> STATE_RATE_SHIFT),
        has_source: encoded & STATE_HAS_SOURCE != 0,
    }
}

fn encode_rate(rate: FrameRate) -> u32 {
    match rate {
        FrameRate::Unlimited => 0,
        FrameRate::Fps120 => 1,
        FrameRate::Fps60 => 2,
        FrameRate::Fps30 => 3,
    }
}

fn decode_rate(rate: u32) -> FrameRate {
    match rate {
        1 => FrameRate::Fps120,
        2 => FrameRate::Fps60,
        3 => FrameRate::Fps30,
        _ => FrameRate::Unlimited,
    }
}

fn action_from_command(command: u32) -> Option<TrayAction> {
    match command {
        CMD_TOGGLE_VISIBILITY => Some(TrayAction::ToggleVisibility),
        CMD_TOGGLE_INTERACTION => Some(TrayAction::ToggleInteraction),
        CMD_TOGGLE_TOPMOST => Some(TrayAction::ToggleTopmost),
        CMD_FPS_UNLIMITED => Some(TrayAction::SetFrameRate(FrameRate::Unlimited)),
        CMD_FPS_120 => Some(TrayAction::SetFrameRate(FrameRate::Fps120)),
        CMD_FPS_60 => Some(TrayAction::SetFrameRate(FrameRate::Fps60)),
        CMD_FPS_30 => Some(TrayAction::SetFrameRate(FrameRate::Fps30)),
        CMD_QUIT => Some(TrayAction::Quit),
        command
            if (CMD_SOURCE_BASE..CMD_SOURCE_BASE + MAX_SOURCE_COMMANDS as u32)
                .contains(&command) =>
        {
            Some(TrayAction::SelectSource(
                (command - CMD_SOURCE_BASE) as usize,
            ))
        }
        _ => None,
    }
}

struct OwnedMenu(HMENU);

impl OwnedMenu {
    fn new() -> Result<Self> {
        unsafe { CreatePopupMenu().map(Self) }
    }

    fn handle(&self) -> HMENU {
        self.0
    }

    fn into_submenu(self) -> usize {
        let handle = self.0 .0 as usize;
        std::mem::forget(self);
        handle
    }
}

impl Drop for OwnedMenu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_state_round_trips() {
        let state = TrayState {
            visible: false,
            interaction: true,
            topmost: true,
            frame_rate: FrameRate::Fps30,
            has_source: true,
        };
        let decoded = decode_state(encode_state(state));
        assert!(!decoded.visible);
        assert!(decoded.interaction);
        assert!(decoded.topmost);
        assert_eq!(decoded.frame_rate, FrameRate::Fps30);
        assert!(decoded.has_source);
    }

    #[test]
    fn frame_rate_commands_map_to_explicit_rates() {
        assert_eq!(
            action_from_command(CMD_FPS_60),
            Some(TrayAction::SetFrameRate(FrameRate::Fps60))
        );
        assert_eq!(action_from_command(0), None);
        assert_eq!(
            action_from_command(CMD_SOURCE_BASE + 3),
            Some(TrayAction::SelectSource(3))
        );
    }
}
