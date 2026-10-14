//! Mouse interaction, drag state and scale constraints. Hotkeys have their own lifetime.
mod hook_thread;
pub mod hotkeys;
use crate::events::EventSender;
pub use hook_thread::{handle_passthrough_event, MouseHook};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, HWND_NOTOPMOST,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};
use winit::{
    dpi::PhysicalPosition,
    event::{ElementState, MouseButton, MouseScrollDelta},
    window::Window,
};

const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 5.0;
const SCALE_STEP: f32 = 0.05;
const MAX_RENDER_DIMENSION: u32 = 16_384;
pub const ALPHA_THRESHOLD: u8 = 10;

pub struct InteractionState {
    pub enabled: bool,
    pub scale_factor: f32,
    pub topmost: bool,
    dragging: bool,
    drag_start_screen: (i32, i32),
    drag_start_origin: (i32, i32),
    mouse_hook: Option<MouseHook>,
    pub mask_epoch: u64,
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
            mouse_hook: None,
            mask_epoch: 0,
        }
    }

    pub fn init_window_style(hwnd: HWND) {
        unsafe {
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
            SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                ex_style | (WS_EX_LAYERED.0 as i32) | (WS_EX_TRANSPARENT.0 as i32),
            );
        }
    }

    pub fn install_mouse_hook(&mut self, hwnd: HWND, events: EventSender) -> Result<(), String> {
        if self.mouse_hook.is_some() {
            return Ok(());
        }
        let hook = MouseHook::install(hwnd, events)?;
        self.mouse_hook = Some(hook);
        Ok(())
    }

    pub fn toggle_enabled(&mut self, hwnd: HWND) {
        self.enabled = !self.enabled;
        if self.enabled {
            self.invalidate_alpha_mask();
        } else {
            self.cancel_drag();
        }
        if let Some(hook) = &self.mouse_hook {
            hook.set_enabled(self.enabled);
        }
        update_transparency(hwnd, self.enabled);
    }

    pub fn toggle_topmost(&mut self, hwnd: HWND) {
        self.topmost = !self.topmost;
        update_topmost(hwnd, self.topmost);
    }

    pub fn handle_scroll(&mut self, delta: MouseScrollDelta) {
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
    }

    pub fn constrain_scale(&mut self, base_w: u32, base_h: u32) {
        let dimension_limit = |value: u32| {
            if value == 0 {
                MAX_SCALE
            } else {
                MAX_RENDER_DIMENSION as f32 / value as f32
            }
        };
        let max_scale = MAX_SCALE
            .min(dimension_limit(base_w))
            .min(dimension_limit(base_h))
            .max(MIN_SCALE);
        self.scale_factor = self.scale_factor.clamp(MIN_SCALE, max_scale);
    }

    pub fn cancel_drag(&mut self) {
        self.dragging = false;
    }

    pub fn invalidate_alpha_mask(&mut self) {
        self.mask_epoch = self.mask_epoch.wrapping_add(1);
        if let Some(hook) = &self.mouse_hook {
            let _ = hook.update_mask(Vec::new(), 0, 0);
        }
    }

    pub fn handle_mouse_input(
        &mut self,
        state: ElementState,
        button: MouseButton,
        window: &Window,
    ) {
        if button != MouseButton::Left {
            return;
        }
        if state == ElementState::Released {
            self.dragging = false;
        }
        if !self.enabled {
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

    pub fn handle_cursor_moved(&mut self, position: PhysicalPosition<f64>, window: &Window) {
        if !self.enabled || !self.dragging {
            return;
        }
        let Ok(outer) = window.outer_position() else {
            return;
        };
        let cur_x = outer.x + position.x as i32;
        let cur_y = outer.y + position.y as i32;
        let dx = cur_x - self.drag_start_screen.0;
        let dy = cur_y - self.drag_start_screen.1;
        let new_x = self.drag_start_origin.0 + dx;
        let new_y = self.drag_start_origin.1 + dy;
        window.set_outer_position(PhysicalPosition::new(new_x, new_y));
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub fn update_alpha_mask(
        &mut self,
        alpha: Vec<u8>,
        width: u32,
        height: u32,
    ) -> Option<Vec<u8>> {
        self.mouse_hook
            .as_ref()
            .and_then(|hook| hook.update_mask(alpha, width, height))
    }

    pub fn update_crop_layout(&self, layout: spout_transparent::crop::CropLayout) {
        if let Some(hook) = &self.mouse_hook {
            hook.set_layout(layout);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_size_never_becomes_zero() {
        let mut state = InteractionState::new();
        state.scale_factor = MIN_SCALE;
        let layout = spout_transparent::crop::CropLayout::new(
            spout_transparent::crop::CropRect::full(1, 1),
            state.scale_factor,
        );
        assert_eq!((layout.width, layout.height), (1, 1));

        state.scale_factor = MAX_SCALE;
        let layout = spout_transparent::crop::CropLayout::new(
            spout_transparent::crop::CropRect::full(u32::MAX, u32::MAX),
            state.scale_factor,
        );
        assert_eq!(
            (layout.width, layout.height),
            (MAX_RENDER_DIMENSION, MAX_RENDER_DIMENSION)
        );
    }

    #[test]
    fn scale_is_constrained_without_changing_aspect_ratio() {
        let mut state = InteractionState::new();
        state.scale_factor = MAX_SCALE;
        state.constrain_scale(3840, 2160);

        let size = spout_transparent::crop::CropLayout::new(
            spout_transparent::crop::CropRect::full(3840, 2160),
            state.scale_factor,
        );
        assert_eq!(size.width, MAX_RENDER_DIMENSION);
        assert!((9216..=9217).contains(&size.height));
        assert!((state.scale_factor - (MAX_RENDER_DIMENSION as f32 / 3840.0)).abs() < 0.001);
    }

    #[test]
    fn disabling_interaction_cancels_dragging() {
        let mut state = InteractionState::new();
        state.enabled = true;
        state.dragging = true;
        state.toggle_enabled(HWND::default());

        assert!(!state.enabled);
        assert!(!state.dragging);
    }
}
