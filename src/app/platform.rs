//! UI-thread COM apartment lifetime. Keep this guard after COM owners in Runtime.
use windows::Win32::System::Com::{CoInitialize, CoUninitialize};

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
