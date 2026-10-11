//! UI snapshot, command mapping and popup-menu ownership. A source command is
//! resolved against the snapshot displayed when the menu opened.
use super::{publish_action, TrayAction};
use crate::{frame_pacer::FrameRate, spout::SenderName};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Mutex,
};
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow,
    TrackPopupMenu, HMENU, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};

const CMD_TOGGLE_VISIBILITY: u32 = 1001;
const CMD_TOGGLE_INTERACTION: u32 = 1002;
const CMD_TOGGLE_TOPMOST: u32 = 1003;
const CMD_TOGGLE_AUTO_CROP: u32 = 1004;
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
const STATE_AUTO_CROP: u32 = 1 << 6;

static MENU_STATE: AtomicU32 = AtomicU32::new(0);
static SOURCE_MENU: Mutex<SourceMenuState> = Mutex::new(SourceMenuState {
    names: Vec::new(),
    selected: None,
});
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayState {
    pub visible: bool,
    pub interaction: bool,
    pub topmost: bool,
    pub frame_rate: FrameRate,
    pub has_source: bool,
    pub auto_crop: bool,
}

#[derive(Clone)]
struct SourceMenuState {
    names: Vec<SenderName>,
    selected: Option<SenderName>,
}

pub(super) fn update_state(state: TrayState) {
    MENU_STATE.store(encode_state(state), Ordering::Release);
}
pub(super) fn update_sources(names: Vec<SenderName>, selected: Option<SenderName>) {
    let mut menu = SOURCE_MENU
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    menu.names = names;
    menu.selected = selected;
}

pub(super) fn show_context_menu(hwnd: HWND) -> Result<()> {
    let state = decode_state(MENU_STATE.load(Ordering::Acquire));
    let sources = SOURCE_MENU
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
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

        if sources.names.is_empty() {
            AppendMenuW(
                source_menu.handle(),
                MF_STRING | MF_GRAYED,
                0,
                w!("无可用来源"),
            )?;
        } else {
            for (index, name) in sources.names.iter().take(MAX_SOURCE_COMMANDS).enumerate() {
                let menu_name = name.display_name().replace('&', "&&");
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
        menu.append_submenu(source_menu, w!("来源"))?;
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
            menu.handle(),
            MF_STRING | checked(state.auto_crop),
            CMD_TOGGLE_AUTO_CROP as usize,
            w!("自动裁切透明边缘"),
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
        menu.append_submenu(frame_menu, w!("帧率"))?;
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
            let action = source_for_command(&sources, command)
                .map(TrayAction::SelectSource)
                .or_else(|| action_from_command(command));
            if let Some(action) = action {
                publish_action(action);
            }
        }
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
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
    if state.auto_crop {
        encoded |= STATE_AUTO_CROP;
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
        auto_crop: encoded & STATE_AUTO_CROP != 0,
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
        CMD_TOGGLE_AUTO_CROP => Some(TrayAction::ToggleAutoCrop),
        CMD_FPS_UNLIMITED => Some(TrayAction::SetFrameRate(FrameRate::Unlimited)),
        CMD_FPS_120 => Some(TrayAction::SetFrameRate(FrameRate::Fps120)),
        CMD_FPS_60 => Some(TrayAction::SetFrameRate(FrameRate::Fps60)),
        CMD_FPS_30 => Some(TrayAction::SetFrameRate(FrameRate::Fps30)),
        CMD_QUIT => Some(TrayAction::Quit),
        _ => None,
    }
}

fn source_for_command(sources: &SourceMenuState, command: u32) -> Option<SenderName> {
    command
        .checked_sub(CMD_SOURCE_BASE)
        .filter(|index| (*index as usize) < MAX_SOURCE_COMMANDS)
        .and_then(|index| sources.names.get(index as usize))
        .cloned()
}

struct OwnedMenu(HMENU);

impl OwnedMenu {
    fn new() -> Result<Self> {
        unsafe { CreatePopupMenu().map(Self) }
    }

    fn handle(&self) -> HMENU {
        self.0
    }

    fn append_submenu(&self, child: Self, label: PCWSTR) -> Result<()> {
        unsafe {
            AppendMenuW(self.0, MF_POPUP, child.0 .0 as usize, label)?;
        }
        // The parent owns the child only after AppendMenuW succeeds.
        std::mem::forget(child);
        Ok(())
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
    fn failed_submenu_attachment_releases_child_menu() -> Result<()> {
        let child = OwnedMenu::new()?;
        let handle = child.handle();
        let invalid_parent = OwnedMenu(HMENU::default());
        assert!(invalid_parent.append_submenu(child, w!("child")).is_err());
        assert!(!unsafe { windows::Win32::UI::WindowsAndMessaging::IsMenu(handle) }.as_bool());
        Ok(())
    }

    #[test]
    fn successful_submenu_attachment_transfers_ownership_to_parent() -> Result<()> {
        let parent = OwnedMenu::new()?;
        let child = OwnedMenu::new()?;
        let handle = child.handle();
        parent.append_submenu(child, w!("child"))?;
        assert!(unsafe { windows::Win32::UI::WindowsAndMessaging::IsMenu(handle) }.as_bool());
        drop(parent);
        assert!(!unsafe { windows::Win32::UI::WindowsAndMessaging::IsMenu(handle) }.as_bool());
        Ok(())
    }

    #[test]
    fn tray_state_round_trips() {
        let state = TrayState {
            visible: false,
            interaction: true,
            topmost: true,
            frame_rate: FrameRate::Fps30,
            has_source: true,
            auto_crop: true,
        };
        let decoded = decode_state(encode_state(state));
        assert!(!decoded.visible);
        assert!(decoded.interaction);
        assert!(decoded.topmost);
        assert_eq!(decoded.frame_rate, FrameRate::Fps30);
        assert!(decoded.has_source);
        assert!(decoded.auto_crop);
    }

    #[test]
    fn frame_rate_commands_map_to_explicit_rates() {
        assert_eq!(
            action_from_command(CMD_FPS_60),
            Some(TrayAction::SetFrameRate(FrameRate::Fps60))
        );
        assert_eq!(action_from_command(0), None);
        assert_eq!(action_from_command(CMD_SOURCE_BASE + 3), None);
    }

    #[test]
    fn source_command_resolves_against_the_visible_menu_snapshot() {
        let sources = SourceMenuState {
            names: vec![SenderName::for_test("Alpha"), SenderName::for_test("Beta")],
            selected: None,
        };

        let selected = source_for_command(&sources, CMD_SOURCE_BASE + 1).unwrap();
        assert_eq!(selected.display_name(), "Beta");
    }
}
