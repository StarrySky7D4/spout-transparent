//! Dedicated tray thread and notification-icon lifetime.
#[path = "tray/menu.rs"]
mod menu;
pub use menu::TrayState;
use std::cell::RefCell;
use std::mem::size_of;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::core::{w, Error, Result};
use windows::Win32::Foundation::{E_FAIL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{
    DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass, Shell_NotifyIconW, NIF_ICON,
    NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, GetWindowLongW, LoadIconW,
    PostMessageW, PostQuitMessage, RegisterWindowMessageW, SetWindowLongW, TranslateMessage,
    GWL_EXSTYLE, IDI_APPLICATION, MSG, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY,
    WM_LBUTTONDBLCLK, WM_RBUTTONUP, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_POPUP,
};

pub use crate::events::AppAction as TrayAction;
use crate::events::{AppEvent, EventSender};
use crate::spout::SenderName;

const TRAY_ICON_ID: u32 = 1;
const TRAY_SUBCLASS_ID: usize = 2;
const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;

static TASKBAR_CREATED_MESSAGE: AtomicU32 = AtomicU32::new(0);
thread_local! {
    static EVENT_SENDER: RefCell<Option<EventSender>> = const { RefCell::new(None) };
}

pub struct TrayIcon {
    hwnd: HWND,
    worker: Option<JoinHandle<()>>,
}

impl TrayIcon {
    pub fn install(render_hwnd: HWND, events: EventSender) -> Result<Self> {
        hide_from_taskbar(render_hwnd);

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("spout-tray".to_string())
            .spawn(move || run_tray_thread(ready_tx, events))
            .map_err(|error| Error::new(E_FAIL, format!("Failed to start tray thread: {error}")))?;

        let hwnd_value = match ready_rx.recv() {
            Ok(Ok(hwnd)) => hwnd,
            Ok(Err(message)) => {
                let _ = worker.join();
                return Err(Error::new(E_FAIL, message));
            }
            Err(error) => {
                let _ = worker.join();
                return Err(Error::new(
                    E_FAIL,
                    format!("Tray thread stopped during startup: {error}"),
                ));
            }
        };

        Ok(Self {
            hwnd: HWND(hwnd_value as *mut std::ffi::c_void),
            worker: Some(worker),
        })
    }

    pub fn update_state(&self, state: TrayState) {
        menu::update_state(state);
    }

    pub fn update_sources(&self, names: Vec<SenderName>, selected: Option<SenderName>) {
        menu::update_sources(names, selected);
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = PostMessageW(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_tray_thread(
    ready: mpsc::SyncSender<std::result::Result<isize, String>>,
    events: EventSender,
) {
    EVENT_SENDER.with(|sender| *sender.borrow_mut() = Some(events));
    let setup = setup_tray_window();
    let hwnd = match setup {
        Ok(hwnd) => hwnd,
        Err(error) => {
            let _ = ready.send(Err(format!("Tray window setup failed: {error:?}")));
            return;
        }
    };

    if ready.send(Ok(hwnd.0 as isize)).is_err() {
        unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if result <= 0 {
            if result < 0 {
                log::error!("Tray message loop failed: {:?}", Error::from_win32());
            }
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn setup_tray_window() -> Result<HWND> {
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("STATIC"),
            w!("Spout Transparent Tray"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )?
    };

    let subclass_ok =
        unsafe { SetWindowSubclass(hwnd, Some(tray_subclass), TRAY_SUBCLASS_ID, 0).as_bool() };
    if !subclass_ok {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return Err(Error::from_win32());
    }

    TASKBAR_CREATED_MESSAGE.store(
        unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        Ordering::Release,
    );

    if let Err(error) = add_icon(hwnd) {
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(tray_subclass), TRAY_SUBCLASS_ID);
            let _ = DestroyWindow(hwnd);
        }
        return Err(error);
    }

    Ok(hwnd)
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
                if let Err(error) = menu::show_context_menu(hwnd) {
                    log::error!("Tray menu failed: {error:?}");
                }
                return LRESULT(0);
            }
            WM_LBUTTONDBLCLK => {
                publish_action(TrayAction::ToggleVisibility);
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

    if message == WM_DESTROY {
        let data = icon_data(hwnd, None);
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        PostQuitMessage(0);
    }

    DefSubclassProc(hwnd, message, wparam, lparam)
}

fn publish_action(action: TrayAction) {
    EVENT_SENDER.with(|sender| {
        if let Some(sender) = sender.borrow().as_ref() {
            let _ = sender.send_event(AppEvent::Action(action));
        }
    });
}
