//! Configuration, registration and UI-thread WM_HOTKEY bridge.
use crate::events::{AppAction, AppEvent, EventSender};
use serde::Deserialize;
use std::{cell::RefCell, path::Path};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};

/// Owns registrations and the subclass on the UI thread. Partial startup failures
/// and normal shutdown use the same cleanup path, before the window is destroyed.
pub struct HotkeySession {
    hwnd: HWND,
}

impl HotkeySession {
    pub fn install(hwnd: HWND, events: EventSender) -> Result<Self, HotkeyError> {
        let session = Self { hwnd };
        let path = if Path::new("hotkeys.json").exists() {
            std::path::PathBuf::from("hotkeys.json")
        } else {
            std::env::current_exe()?
                .parent()
                .ok_or_else(|| HotkeyError::Config("No executable directory".into()))?
                .join("hotkeys.json")
        };
        if path.exists() {
            load_hotkey_config(&path, hwnd)?;
        } else {
            register_default_hotkeys(hwnd)?;
        }
        install_hotkey_subclass(hwnd, events)?;
        Ok(session)
    }
}

impl Drop for HotkeySession {
    fn drop(&mut self) {
        uninstall_hotkey_subclass(self.hwnd);
        unregister_all_hotkeys(self.hwnd);
    }
}

#[link(name = "user32")]
extern "system" {
    fn RegisterHotKey(hWnd: HWND, id: i32, fsModifiers: u32, vk: u32) -> i32;
    fn UnregisterHotKey(hWnd: HWND, id: i32) -> i32;
}

const MOD_NOREPEAT_FLAG: u32 = 0x4000;
const WM_HOTKEY_MSG: u32 = 0x0312;

const DEFAULT_HOTKEY_CONFIG: &str = r#"{"hotkeys":[{"modifiers":["CTRL","SHIFT"],"key":"M","action":"toggle_interaction"},{"modifiers":["CTRL","SHIFT"],"key":"F","action":"cycle_framerate"},{"modifiers":["CTRL","SHIFT"],"key":"T","action":"toggle_topmost"},{"modifiers":["CTRL","SHIFT"],"key":"Q","action":"quit"}]}"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotKeyAction {
    ToggleInteraction,
    ToggleTopmost,
    Quit,
    CycleFrameRate,
}

#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error("RegisterHotKey failed for id {id}")]
    HotKeyRegistrationFailed { id: i32 },
    #[error("SetWindowSubclass failed")]
    SubclassInstallFailed,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Config error: {0}")]
    Config(String),
}

struct ActiveHotkey {
    id: i32,
    action: HotKeyAction,
}

static ACTIVE_HOTKEYS: std::sync::RwLock<Vec<ActiveHotkey>> = std::sync::RwLock::new(Vec::new());
thread_local! {
    static EVENT_SENDER: RefCell<Option<EventSender>> = const { RefCell::new(None) };
}

#[derive(Deserialize)]
struct HotKeyConfig {
    hotkeys: Vec<RawHotKeyDef>,
}

#[derive(Deserialize)]
struct RawHotKeyDef {
    modifiers: Vec<String>,
    key: String,
    action: String,
}

#[derive(Debug)]
struct ParsedHotkey {
    id: i32,
    modifiers: u32,
    virtual_key: u32,
    action: HotKeyAction,
}

fn parse_modifiers(mods: &[String]) -> Option<u32> {
    let mut flags = 0u32;
    for m in mods {
        match m.to_uppercase().as_str() {
            "CTRL" | "CONTROL" => flags |= MOD_CONTROL.0,
            "SHIFT" => flags |= MOD_SHIFT.0,
            "ALT" => flags |= MOD_ALT.0,
            _ => return None,
        }
    }
    Some(flags | MOD_NOREPEAT_FLAG)
}

fn parse_vk(key: &str) -> Option<u32> {
    let upper = key.to_uppercase();
    let mut chars = upper.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if c.is_ascii_alphabetic() || c.is_ascii_digit() {
        Some(c as u32)
    } else {
        None
    }
}

fn parse_action(action: &str) -> Option<HotKeyAction> {
    match action {
        "toggle_interaction" => Some(HotKeyAction::ToggleInteraction),
        "toggle_topmost" => Some(HotKeyAction::ToggleTopmost),
        "quit" => Some(HotKeyAction::Quit),
        "cycle_framerate" => Some(HotKeyAction::CycleFrameRate),
        _ => None,
    }
}

fn parse_config(config_str: &str) -> Result<Vec<ParsedHotkey>, HotkeyError> {
    let config: HotKeyConfig =
        serde_json::from_str(config_str).map_err(|e| HotkeyError::Config(e.to_string()))?;
    let mut parsed_hotkeys = Vec::with_capacity(config.hotkeys.len());
    for (i, raw) in config.hotkeys.iter().enumerate() {
        let mod_flags = parse_modifiers(&raw.modifiers)
            .ok_or_else(|| HotkeyError::Config(format!("Invalid modifiers in hotkey {i}")))?;
        let vk = parse_vk(&raw.key).ok_or_else(|| {
            HotkeyError::Config(format!("Unknown key '{}' in hotkey {}", raw.key, i))
        })?;
        let action = parse_action(&raw.action).ok_or_else(|| {
            HotkeyError::Config(format!("Unknown action '{}' in hotkey {}", raw.action, i))
        })?;
        let id =
            i32::try_from(i + 1).map_err(|_| HotkeyError::Config("Too many hotkeys".into()))?;
        parsed_hotkeys.push(ParsedHotkey {
            id,
            modifiers: mod_flags,
            virtual_key: vk,
            action,
        });
    }

    Ok(parsed_hotkeys)
}

fn parse_and_register(config_str: &str, hwnd: HWND) -> Result<(), HotkeyError> {
    let parsed_hotkeys = parse_config(config_str)?;
    unregister_all_hotkeys(hwnd);
    let mut new_hotkeys = Vec::with_capacity(parsed_hotkeys.len());
    let mut registered_ids = Vec::with_capacity(parsed_hotkeys.len());
    for ParsedHotkey {
        id,
        modifiers,
        virtual_key,
        action,
    } in parsed_hotkeys
    {
        let res = unsafe { RegisterHotKey(hwnd, id, modifiers, virtual_key) };
        if res == 0 {
            for rid in &registered_ids {
                let _ = unsafe { UnregisterHotKey(hwnd, *rid) };
            }
            return Err(HotkeyError::HotKeyRegistrationFailed { id });
        }
        registered_ids.push(id);
        new_hotkeys.push(ActiveHotkey { id, action });
    }
    if let Ok(mut guard) = ACTIVE_HOTKEYS.write() {
        *guard = new_hotkeys;
    }
    Ok(())
}

fn load_hotkey_config(path: &Path, hwnd: HWND) -> Result<(), HotkeyError> {
    let content = std::fs::read_to_string(path)?;
    parse_and_register(&content, hwnd)
}

fn register_default_hotkeys(hwnd: HWND) -> Result<(), HotkeyError> {
    parse_and_register(DEFAULT_HOTKEY_CONFIG, hwnd)
}

unsafe extern "system" fn global_hotkey_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uid: usize,
    _refdata: usize,
) -> LRESULT {
    if msg == WM_HOTKEY_MSG {
        let id = wparam.0 as i32;
        if let Ok(guard) = ACTIVE_HOTKEYS.read() {
            if let Some(hotkey) = guard.iter().find(|h| h.id == id) {
                let action = match hotkey.action {
                    HotKeyAction::ToggleInteraction => AppAction::ToggleInteraction,
                    HotKeyAction::ToggleTopmost => AppAction::ToggleTopmost,
                    HotKeyAction::Quit => AppAction::Quit,
                    HotKeyAction::CycleFrameRate => AppAction::CycleFrameRate,
                };
                EVENT_SENDER.with(|sender| {
                    if let Some(sender) = sender.borrow().as_ref() {
                        let _ = sender.send_event(AppEvent::Action(action));
                    }
                });
            }
        }
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

fn install_hotkey_subclass(hwnd: HWND, events: EventSender) -> Result<(), HotkeyError> {
    EVENT_SENDER.with(|sender| *sender.borrow_mut() = Some(events));
    let res = unsafe { SetWindowSubclass(hwnd, Some(global_hotkey_subclass), 1, 0) };
    if res.as_bool() {
        Ok(())
    } else {
        Err(HotkeyError::SubclassInstallFailed)
    }
}

fn uninstall_hotkey_subclass(hwnd: HWND) {
    EVENT_SENDER.with(|sender| *sender.borrow_mut() = None);
    unsafe {
        let _ = RemoveWindowSubclass(hwnd, Some(global_hotkey_subclass), 1);
    }
}

fn unregister_all_hotkeys(hwnd: HWND) {
    let mut ids: Vec<i32> = Vec::new();
    if let Ok(guard) = ACTIVE_HOTKEYS.read() {
        for hotkey in guard.iter() {
            ids.push(hotkey.id);
        }
    }
    for id in ids {
        let _ = unsafe { UnregisterHotKey(hwnd, id) };
    }
    if let Ok(mut guard) = ACTIVE_HOTKEYS.write() {
        guard.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_preserves_registered_actions_and_key_combinations() {
        let hotkeys = parse_config(DEFAULT_HOTKEY_CONFIG).unwrap();
        let expected = [
            ('M', HotKeyAction::ToggleInteraction),
            ('F', HotKeyAction::CycleFrameRate),
            ('T', HotKeyAction::ToggleTopmost),
            ('Q', HotKeyAction::Quit),
        ];
        assert_eq!(hotkeys.len(), expected.len());
        for (index, (hotkey, (key, action))) in hotkeys.iter().zip(expected).enumerate() {
            assert_eq!(hotkey.id, index as i32 + 1);
            assert_eq!(hotkey.virtual_key, key as u32);
            assert_eq!(hotkey.action, action);
            assert_eq!(
                hotkey.modifiers,
                MOD_CONTROL.0 | MOD_SHIFT.0 | MOD_NOREPEAT_FLAG
            );
        }
    }

    #[test]
    fn invalid_config_is_rejected_before_registration() {
        for (field, value) in [
            ("modifiers", serde_json::json!(["WIN"])),
            ("key", serde_json::json!("AB")),
            ("action", serde_json::json!("unknown")),
        ] {
            let mut config: serde_json::Value =
                serde_json::from_str(DEFAULT_HOTKEY_CONFIG).unwrap();
            config["hotkeys"][3][field] = value;
            assert!(matches!(
                parse_config(&config.to_string()),
                Err(HotkeyError::Config(_))
            ));
        }
    }

    #[test]
    fn modifiers_accept_control_alias_and_ignore_case() {
        assert_eq!(
            parse_modifiers(&["control".into(), "Alt".into()]),
            Some(MOD_CONTROL.0 | MOD_ALT.0 | MOD_NOREPEAT_FLAG)
        );
    }

    #[test]
    fn virtual_key_requires_exactly_one_character() {
        assert_eq!(parse_vk("a"), Some('A' as u32));
        assert_eq!(parse_vk("7"), Some('7' as u32));
        assert_eq!(parse_vk("AB"), None);
        assert_eq!(parse_vk(""), None);
    }
}
