use windows::Win32::Graphics::Dxgi::IDXGIKeyedMutex;

pub struct KeyedMutexGuard<'a> {
    mutex: &'a IDXGIKeyedMutex,
    acquired: bool,
}

impl<'a> KeyedMutexGuard<'a> {
    pub fn try_acquire(mutex: &'a IDXGIKeyedMutex) -> Option<Self> {
        let acquired = unsafe { mutex.AcquireSync(0, 0) }.is_ok();
        if acquired {
            Some(Self { mutex, acquired: true })
        } else {
            None
        }
    }
}

impl Drop for KeyedMutexGuard<'_> {
    fn drop(&mut self) {
        if self.acquired {
            let _ = unsafe { self.mutex.ReleaseSync(1) };
        }
    }
}
