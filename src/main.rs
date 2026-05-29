#![windows_subsystem = "windows"]

mod app;
mod config;
mod dx;
mod interaction;
mod spout_util;

#[cfg(debug_assertions)]
fn attach_console() {
    use windows::Win32::System::Console::{
        AllocConsole, GetConsoleMode, GetStdHandle, SetConsoleMode, CONSOLE_MODE,
        STD_OUTPUT_HANDLE,
    };
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    unsafe {
        let _ = AllocConsole();
        if let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) {
            let mut mode = CONSOLE_MODE(0);
            if GetConsoleMode(handle, &mut mode).is_ok() {
                let _ = SetConsoleMode(
                    handle,
                    CONSOLE_MODE(mode.0 | ENABLE_VIRTUAL_TERMINAL_PROCESSING),
                );
            }
        }
    }
}

#[cfg(not(debug_assertions))]
fn attach_console() {}

fn main() {
    attach_console();
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();
    if let Err(e) = app::run() {
        log::error!("Fatal error: {e:#}");
        std::process::exit(1);
    }
}
