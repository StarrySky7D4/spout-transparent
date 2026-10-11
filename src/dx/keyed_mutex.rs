use windows::core::Interface;
use windows::Win32::Graphics::Dxgi::IDXGIKeyedMutex;

pub struct KeyedMutexGuard<'a> {
    mutex: &'a IDXGIKeyedMutex,
    acquired: bool,
}

impl<'a> KeyedMutexGuard<'a> {
    pub fn try_acquire(mutex: &'a IDXGIKeyedMutex) -> Option<Self> {
        // The generated Result<()> wrapper loses positive WAIT_TIMEOUT and
        // WAIT_ABANDONED values. Only the exact S_OK result owns the texture.
        let result =
            unsafe { (Interface::vtable(mutex).AcquireSync)(Interface::as_raw(mutex), 0, 0) };
        let acquired = result.0 == 0;
        if acquired {
            Some(Self {
                mutex,
                acquired: true,
            })
        } else {
            None
        }
    }
}

impl Drop for KeyedMutexGuard<'_> {
    fn drop(&mut self) {
        if self.acquired {
            let _ = unsafe { self.mutex.ReleaseSync(0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Requires a Windows D3D11 device"]
    fn timeout_does_not_create_a_texture_guard() -> windows::core::Result<()> {
        use windows::Win32::Graphics::Direct3D11::*;
        use windows::Win32::Graphics::Dxgi::Common::*;
        let (device, _) = crate::dx::device::create_dx11_device_auto()?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: 2,
            Height: 2,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
            ..Default::default()
        };
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
        let mutex: IDXGIKeyedMutex = texture.unwrap().cast()?;
        let guard = KeyedMutexGuard::try_acquire(&mutex).expect("initial key 0 is available");
        drop(guard);
        unsafe {
            mutex.AcquireSync(0, 0)?;
            mutex.ReleaseSync(7)?;
        }
        assert!(KeyedMutexGuard::try_acquire(&mutex).is_none());
        let raw =
            unsafe { (Interface::vtable(&mutex).AcquireSync)(Interface::as_raw(&mutex), 7, 0) };
        assert_eq!(
            raw.0, 0,
            "failed acquisition must not release someone else's key"
        );
        unsafe {
            mutex.ReleaseSync(0)?;
        }
        Ok(())
    }
}
