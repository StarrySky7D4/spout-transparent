//! Nonblocking frame-count probe with the legacy-sender compatibility fallback.
use super::{shared_memory::OwnedHandle, SenderName};
use std::{io, ptr};
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{
    CreateSemaphoreA, ReleaseSemaphore, WaitForSingleObject,
};

pub struct FrameCounter {
    handle: OwnedHandle,
    last_count: i32,
}

impl FrameCounter {
    pub fn for_sender(sender: &SenderName) -> io::Result<Self> {
        let name = sender.with_suffix(b"_Count_Semaphore")?;
        // Spout deliberately lets either endpoint create the semaphore. A count
        // of zero after the probe means the sender is not using frame counting.
        let handle = unsafe { CreateSemaphoreA(ptr::null(), 1, i32::MAX, name.as_ptr().cast()) };
        Ok(Self {
            handle: OwnedHandle::new(handle)?,
            last_count: 0,
        })
    }

    pub fn is_new_frame(&mut self) -> bool {
        match unsafe { WaitForSingleObject(self.handle.0, 0) } {
            WAIT_OBJECT_0 => {
                let mut count_after_wait = 0;
                if unsafe { ReleaseSemaphore(self.handle.0, 1, &mut count_after_wait) } == 0 {
                    return true;
                }
                frame_count_is_new(&mut self.last_count, count_after_wait)
            }
            // A broken or concurrently unavailable counter must not freeze a
            // receiver. This is the same compatibility fallback used by Spout.
            _ => true,
        }
    }
}

fn frame_count_is_new(last_count: &mut i32, current_count: i32) -> bool {
    if current_count == 0 {
        return true;
    }
    if current_count == *last_count {
        return false;
    }
    *last_count = current_count;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_frame_count_keeps_legacy_senders_live() {
        let mut last = 0;
        assert!(frame_count_is_new(&mut last, 0));
        assert!(frame_count_is_new(&mut last, 0));
        assert_eq!(last, 0);
    }

    #[test]
    fn repeated_nonzero_frame_count_is_not_new() {
        let mut last = 0;
        assert!(frame_count_is_new(&mut last, 7));
        assert!(!frame_count_is_new(&mut last, 7));
        assert!(frame_count_is_new(&mut last, 9));
    }
}
