use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetWindowLongW, GetWindowRect,
    PostThreadMessageW, SetWindowLongW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WindowFromPoint,
    GWL_EXSTYLE, MSG, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WS_EX_TRANSPARENT,
};

use super::ALPHA_THRESHOLD;

pub struct PassthroughEvent {
    pub msg: u32,
    pub wparam: WPARAM,
    pub screen_pt: POINT,
}

enum HookCommand {
    UpdateMask(Arc<[u8]>, u32, u32),
    SetEnabled(bool),
    Shutdown,
}

static HOOK_THREAD_ID: AtomicUsize = AtomicUsize::new(0);

struct HookThreadState {
    hwnd: HWND,
    mask_alpha: Arc<[u8]>,
    mask_width: u32,
    mask_height: u32,
    enabled: bool,
    evt_tx: Sender<PassthroughEvent>,
}

pub struct MouseHook {
    cmd_tx: Sender<HookCommand>,
    thread_handle: Option<JoinHandle<()>>,
    evt_rx: Receiver<PassthroughEvent>,
}

impl MouseHook {
    pub fn install(hwnd: HWND) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<HookCommand>();
        let (evt_tx, evt_rx) = mpsc::channel::<PassthroughEvent>();

        // Store hwnd as raw value for cross-thread safety
        let hwnd_raw = hwnd.0 as isize;

        let handle = thread::Builder::new()
            .name("mouse_hook".into())
            .spawn(move || {
                let hwnd = HWND(hwnd_raw as *mut _);
                hook_thread_entry(hwnd, cmd_rx, evt_tx);
            })
            .map_err(|e| format!("Failed to spawn hook thread: {e}"))?;

        Ok(Self {
            cmd_tx,
            thread_handle: Some(handle),
            evt_rx,
        })
    }

    pub fn update_mask(&self, alpha: Arc<[u8]>, width: u32, height: u32) {
        let _ = self
            .cmd_tx
            .send(HookCommand::UpdateMask(alpha, width, height));
    }

    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.cmd_tx.send(HookCommand::SetEnabled(enabled));
    }

    pub fn poll_events(&self) -> impl Iterator<Item = PassthroughEvent> + '_ {
        std::iter::from_fn(|| self.evt_rx.try_recv().ok())
    }

    pub fn uninstall(&mut self) {
        let _ = self.cmd_tx.send(HookCommand::Shutdown);
        let tid = HOOK_THREAD_ID.load(Ordering::Acquire);
        if tid != 0 {
            let _ = unsafe { PostThreadMessageW(tid as u32, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
        HOOK_THREAD_ID.store(0, Ordering::Release);
    }
}

impl Drop for MouseHook {
    fn drop(&mut self) {
        self.uninstall();
    }
}

fn hook_thread_entry(
    hwnd: HWND,
    cmd_rx: Receiver<HookCommand>,
    evt_tx: Sender<PassthroughEvent>,
) {
    HOOK_THREAD_ID.store(
        unsafe { windows::Win32::System::Threading::GetCurrentThreadId() } as usize,
        Ordering::Release,
    );

    let mut state = HookThreadState {
        hwnd,
        mask_alpha: Arc::new([]),
        mask_width: 0,
        mask_height: 0,
        enabled: false,
        evt_tx,
    };

    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(hook_callback), None, 0) };

    let hook = match hook {
        Ok(h) => h,
        Err(e) => {
            log::error!("SetWindowsHookExW failed in hook thread: {e:?}");
            return;
        }
    };

    let state_ptr = &mut state as *mut HookThreadState;
    HOOK_STATE_PTR.store(state_ptr as usize, Ordering::Release);

    let mut msg = MSG::default();
    loop {
        // Process pending commands
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                HookCommand::UpdateMask(alpha, w, h) => {
                    state.mask_alpha = alpha;
                    state.mask_width = w;
                    state.mask_height = h;
                }
                HookCommand::SetEnabled(e) => {
                    state.enabled = e;
                }
                HookCommand::Shutdown => {
                    unsafe {
                        let _ = UnhookWindowsHookEx(hook);
                        HOOK_STATE_PTR.store(0, Ordering::Release);
                    }
                    return;
                }
            }
        }

        unsafe {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if !ret.as_bool() || msg.message == WM_QUIT {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        let _ = UnhookWindowsHookEx(hook);
        HOOK_STATE_PTR.store(0, Ordering::Release);
    }
}

static HOOK_STATE_PTR: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn hook_callback(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let msg = wparam.0 as u32;
    let is_button = matches!(
        msg,
        WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_RBUTTONDOWN
            | WM_RBUTTONUP
            | WM_MBUTTONDOWN
            | WM_MBUTTONUP
            | WM_MOUSEWHEEL
    );
    if !is_button {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let state_ptr = HOOK_STATE_PTR.load(Ordering::Acquire) as *mut HookThreadState;
    if state_ptr.is_null() {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let state = &*state_ptr;

    if !state.enabled {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let hook_data =
        &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::MSLLHOOKSTRUCT);
    let screen_pt = hook_data.pt;

    if state.mask_width == 0 || state.mask_height == 0 || state.mask_alpha.is_empty() {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let hwnd_under = unsafe { WindowFromPoint(screen_pt) };
    if hwnd_under != state.hwnd {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let mut rect = windows::Win32::Foundation::RECT::default();
    unsafe {
        let _ = GetWindowRect(state.hwnd, &mut rect);
    }
    let cx = screen_pt.x - rect.left;
    let cy = screen_pt.y - rect.top;
    if cx < 0 || cy < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let win_w = (rect.right - rect.left) as u32;
    let win_h = (rect.bottom - rect.top) as u32;
    if win_w == 0 || win_h == 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let px = (cx as u32 * state.mask_width) / win_w;
    let py = (cy as u32 * state.mask_height) / win_h;
    if px >= state.mask_width || py >= state.mask_height {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let idx = (py * state.mask_width + px) as usize;
    let is_transparent =
        idx < state.mask_alpha.len() && state.mask_alpha[idx] < ALPHA_THRESHOLD;

    if is_transparent {
        let _ = state.evt_tx.send(PassthroughEvent {
            msg,
            wparam,
            screen_pt,
        });
        return LRESULT(1);
    }

    CallNextHookEx(None, code, wparam, lparam)
}

pub fn handle_passthrough_event(ev: &PassthroughEvent, hwnd: HWND) {
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
    unsafe {
        SetWindowLongW(
            hwnd,
            GWL_EXSTYLE,
            ex_style | (WS_EX_TRANSPARENT.0 as i32),
        );
    }
    let below = unsafe { WindowFromPoint(ev.screen_pt) };
    unsafe {
        SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style);
    }
    if below != HWND::default() && below != hwnd {
        let mut mapped = ev.screen_pt;
        unsafe { let _ = windows::Win32::Graphics::Gdi::ScreenToClient(below, &mut mapped); }
        if mapped.x >= 0 && mapped.y >= 0 {
            let new_lparam =
                LPARAM((((mapped.y as u16 as u32) << 16) | (mapped.x as u16 as u32)) as isize);
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                    below,
                    ev.msg,
                    ev.wparam,
                    new_lparam,
                );
            }
        }
    }
}
