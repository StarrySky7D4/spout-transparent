mod hook_thread;

use serde::Deserialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
pub use hook_thread::{MouseHook, PassthroughEvent, handle_passthrough_event};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowLongW, SetWindowLongW, SetWindowPos,
    GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::keyboard::ModifiersState;
use winit::window::Window;

#[link(name = "user32")]
extern "system" {
    fn RegisterHotKey(hWnd: HWND, id: i32, fsModifiers: u32, vk: u32) -> i32;
    fn UnregisterHotKey(hWnd: HWND, id: i32) -> i32;
}

const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 5.0;
const SCALE_STEP: f32 = 0.05;
pub const ALPHA_UPDATE_INTERVAL: u32 = 15;
pub const ALPHA_THRESHOLD: u8 = 10;
const WM_HOTKEY_MSG: u32 = 0x0312;

const DEFAULT_HOTKEY_CONFIG: &str = r#"{"hotkeys":[{"modifiers":["CTRL","SHIFT"],"key":"M","action":"toggle_interaction"},{"modifiers":["CTRL","SHIFT"],"key":"F","action":"cycle_framerate"},{"modifiers":["CTRL","SHIFT"],"key":"T","action":"toggle_topmost"},{"modifiers":["CTRL","SHIFT"],"key":"Q","action":"quit"}]}"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotKeyAction {
    ToggleInteraction,
    ToggleTopmost,
    Quit,
    CycleFrameRate,
}

#[derive(Debug, thiserror::Error)]
pub enum InteractionError {
    #[error("RegisterHotKey failed for id {id}")]
    HotKeyRegistrationFailed { id: i32 },
    #[error("SetWindowSubclass failed")]
    SubclassInstallFailed,
    #[error("Win32 error: {0:?}")]
    Win32(#[from] windows::core::Error),
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
static TOGGLE_INTERACTION_REQUESTED: AtomicBool = AtomicBool::new(false);
static TOGGLE_TOPMOST_REQUESTED: AtomicBool = AtomicBool::new(false);
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static CYCLE_FRAMERATE_REQUESTED: AtomicBool = AtomicBool::new(false);

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
    Some(flags)
}

fn parse_vk(key: &str) -> Option<u32> {
    let upper = key.to_uppercase();
    let c = upper.chars().next()?;
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

fn parse_and_register(config_str: &str, hwnd: HWND) -> Result<(), InteractionError> {
    let config: HotKeyConfig =
        serde_json::from_str(config_str).map_err(|e| InteractionError::Config(e.to_string()))?;
    unregister_all_hotkeys(hwnd);
    let mut new_hotkeys = Vec::new();
    let mut registered_ids = Vec::new();
    for (i, raw) in config.hotkeys.iter().enumerate() {
        let mod_flags = parse_modifiers(&raw.modifiers).ok_or_else(|| {
            InteractionError::Config(format!("Invalid modifiers in hotkey {i}"))
        })?;
        let vk = parse_vk(&raw.key).ok_or_else(|| {
            InteractionError::Config(format!("Unknown key '{}' in hotkey {}", raw.key, i))
        })?;
        let action = parse_action(&raw.action).ok_or_else(|| {
            InteractionError::Config(format!("Unknown action '{}' in hotkey {}", raw.action, i))
        })?;
        let id = (i as i32) + 1;
        let res = unsafe { RegisterHotKey(hwnd, id, mod_flags, vk) };
        if res == 0 {
            for rid in &registered_ids {
                let _ = unsafe { UnregisterHotKey(hwnd, *rid) };
            }
            return Err(InteractionError::HotKeyRegistrationFailed { id });
        }
        registered_ids.push(id);
        new_hotkeys.push(ActiveHotkey { id, action });
    }
    if let Ok(mut guard) = ACTIVE_HOTKEYS.write() {
        *guard = new_hotkeys;
    }
    Ok(())
}

pub fn load_hotkey_config(path: &Path, hwnd: HWND) -> Result<(), InteractionError> {
    let content = std::fs::read_to_string(path)?;
    parse_and_register(&content, hwnd)
}

pub fn register_default_hotkeys(hwnd: HWND) -> Result<(), InteractionError> {
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
                match hotkey.action {
                    HotKeyAction::ToggleInteraction => {
                        TOGGLE_INTERACTION_REQUESTED.store(true, Ordering::Release)
                    }
                    HotKeyAction::ToggleTopmost => {
                        TOGGLE_TOPMOST_REQUESTED.store(true, Ordering::Release)
                    }
                    HotKeyAction::Quit => QUIT_REQUESTED.store(true, Ordering::Release),
                    HotKeyAction::CycleFrameRate => {
                        CYCLE_FRAMERATE_REQUESTED.store(true, Ordering::Release)
                    }
                }
            }
        }
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

pub fn install_hotkey_subclass(hwnd: HWND) -> Result<(), InteractionError> {
    let res = unsafe { SetWindowSubclass(hwnd, Some(global_hotkey_subclass), 1, 0) };
    if res.as_bool() {
        Ok(())
    } else {
        Err(InteractionError::SubclassInstallFailed)
    }
}

pub fn uninstall_hotkey_subclass(hwnd: HWND) {
    unsafe {
        let _ = RemoveWindowSubclass(hwnd, Some(global_hotkey_subclass), 1);
    }
}

pub fn poll_toggle_interaction() -> bool {
    TOGGLE_INTERACTION_REQUESTED.swap(false, Ordering::Acquire)
}

pub fn poll_toggle_topmost() -> bool {
    TOGGLE_TOPMOST_REQUESTED.swap(false, Ordering::Acquire)
}

pub fn poll_quit() -> bool {
    QUIT_REQUESTED.swap(false, Ordering::Acquire)
}

pub fn poll_cycle_framerate() -> bool {
    CYCLE_FRAMERATE_REQUESTED.swap(false, Ordering::Acquire)
}

pub fn unregister_all_hotkeys(hwnd: HWND) {
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

pub struct InteractionState {
    pub enabled: bool,
    pub scale_factor: f32,
    pub topmost: bool,
    dragging: bool,
    drag_start_screen: (i32, i32),
    drag_start_origin: (i32, i32),
    modifiers: ModifiersState,
    frame_counter: u32,
    mouse_hook: Option<MouseHook>,
}

impl InteractionState {
    pub fn new() -> Self {
        Self {
            enabled: false,
            scale_factor: 1.0,
            topmost: false,
            dragging: false,
            drag_start_screen: (0, 0),
            drag_start_origin: (0, 0),
            modifiers: ModifiersState::default(),
            frame_counter: 0,
            mouse_hook: None,
        }
    }

    pub fn init_window_style(hwnd: HWND) {
        unsafe {
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
            SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                ex_style
                    | (WS_EX_LAYERED.0 as i32)
                    | (WS_EX_TRANSPARENT.0 as i32),
            );
        }
    }

    pub fn install_mouse_hook(&mut self, hwnd: HWND) -> Result<(), String> {
        if self.mouse_hook.is_some() {
            return Ok(());
        }
        let hook = MouseHook::install(hwnd)?;
        self.mouse_hook = Some(hook);
        Ok(())
    }

    pub fn poll_hook_events(&self) -> Vec<PassthroughEvent> {
        if let Some(hook) = &self.mouse_hook {
            hook.poll_events().collect()
        } else {
            Vec::new()
        }
    }

    pub fn cleanup(&mut self, hwnd: HWND) {
        self.mouse_hook = None;
        uninstall_hotkey_subclass(hwnd);
        unregister_all_hotkeys(hwnd);
    }

    pub fn update_modifiers(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
    }

    pub fn handle_keyboard(&mut self, event: &KeyEvent, _hwnd: HWND) {
        if !self.enabled {
            return;
        }
        if event.state != ElementState::Pressed {
            return;
        }
        let _ = event;
    }

    pub fn toggle_enabled(&mut self, hwnd: HWND) {
        self.enabled = !self.enabled;
        if let Some(hook) = &self.mouse_hook {
            hook.set_enabled(self.enabled);
        }
        update_transparency(hwnd, self.enabled);
    }

    pub fn toggle_topmost(&mut self, hwnd: HWND) {
        self.topmost = !self.topmost;
        update_topmost(hwnd, self.topmost);
    }

    pub fn handle_scroll(
        &mut self,
        delta: MouseScrollDelta,
        window: &Window,
        base_w: f32,
        base_h: f32,
    ) {
        if !self.enabled {
            return;
        }
        let delta_y = match delta {
            MouseScrollDelta::LineDelta(_, y) => y,
            MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 40.0,
        };
        if delta_y > 0.0 {
            self.scale_factor = (self.scale_factor * (1.0 + SCALE_STEP)).min(MAX_SCALE);
        } else if delta_y < 0.0 {
            self.scale_factor = (self.scale_factor / (1.0 + SCALE_STEP)).max(MIN_SCALE);
        }
        let new_w = (base_w * self.scale_factor) as u32;
        let new_h = (base_h * self.scale_factor) as u32;
        let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(new_w, new_h));
    }

    pub fn handle_mouse_input(
        &mut self,
        state: ElementState,
        button: MouseButton,
        window: &Window,
    ) {
        if !self.enabled || button != MouseButton::Left {
            return;
        }
        match state {
            ElementState::Pressed => {
                self.dragging = true;
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                }
                self.drag_start_screen = (pt.x, pt.y);
                if let Ok(pos) = window.outer_position() {
                    self.drag_start_origin = (pos.x, pos.y);
                }
            }
            ElementState::Released => self.dragging = false,
        }
    }

    pub fn handle_cursor_moved(
        &mut self,
        position: PhysicalPosition<f64>,
        window: &Window,
    ) {
        if !self.enabled || !self.dragging {
            return;
        }
        let Ok(outer) = window.outer_position() else { return };
        let cur_x = outer.x + position.x as i32;
        let cur_y = outer.y + position.y as i32;
        let dx = cur_x - self.drag_start_screen.0;
        let dy = cur_y - self.drag_start_screen.1;
        let new_x = self.drag_start_origin.0 + dx;
        let new_y = self.drag_start_origin.1 + dy;
        window.set_outer_position(PhysicalPosition::new(new_x, new_y));
    }

    pub fn should_update_alpha(&mut self) -> bool {
        self.frame_counter += 1;
        if self.frame_counter >= ALPHA_UPDATE_INTERVAL {
            self.frame_counter = 0;
            return self.enabled;
        }
        false
    }

    pub fn update_alpha_mask(&mut self, alpha: Vec<u8>, width: u32, height: u32) {
        let arc: Arc<[u8]> = alpha.into_boxed_slice().into();
        if let Some(hook) = &self.mouse_hook {
            hook.update_mask(arc, width, height);
        }
    }
}

fn update_transparency(hwnd: HWND, enabled: bool) {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let layered = WS_EX_LAYERED.0 as i32;
        let transparent = WS_EX_TRANSPARENT.0 as i32;
        if enabled {
            SetWindowLongW(hwnd, GWL_EXSTYLE, (ex_style | layered) & !transparent);
        } else {
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style | layered | transparent);
        }
    }
}

fn update_topmost(hwnd: HWND, topmost: bool) {
    unsafe {
        let insert_after = if topmost {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        let _ = SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}
