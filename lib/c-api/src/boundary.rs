use crate::types::*;
use spout_transparent::{spout::SenderName, SpoutReceiver};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{c_char, CString},
    io,
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};

pub struct Error {
    pub status: i32,
    pub native: i32,
    pub message: String,
}
impl Error {
    pub fn argument(message: &str) -> Self {
        Self {
            status: INVALID_ARGUMENT,
            native: 0,
            message: message.into(),
        }
    }
    pub fn handle() -> Self {
        Self {
            status: INVALID_HANDLE,
            native: 0,
            message: "Invalid, stale, wrong-type or foreign-thread handle".into(),
        }
    }
    pub fn not_found(message: &str) -> Self {
        Self {
            status: NOT_FOUND,
            native: 0,
            message: message.into(),
        }
    }
    pub fn pending() -> Self {
        Self {
            status: WOULD_BLOCK,
            native: 0,
            message: "No completed alpha sample".into(),
        }
    }
    pub fn gpu(error: impl std::error::Error + 'static) -> Self {
        let mut cause: &dyn std::error::Error = &error;
        let native = loop {
            if let Some(error) = cause.downcast_ref::<spout_transparent::windows::core::Error>() {
                break error.code().0;
            }
            if let Some(error) = cause.downcast_ref::<io::Error>() {
                break error.raw_os_error().unwrap_or(0);
            }
            match cause.source() {
                Some(source) => cause = source,
                None => break 0,
            }
        };
        Self {
            status: GPU_ERROR,
            native,
            message: error.to_string(),
        }
    }
}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self {
            status: match error.kind() {
                io::ErrorKind::InvalidInput => INVALID_ARGUMENT,
                io::ErrorKind::NotFound => NOT_FOUND,
                io::ErrorKind::WouldBlock => WOULD_BLOCK,
                _ => IO_ERROR,
            },
            native: error.raw_os_error().unwrap_or(0),
            message: error.to_string(),
        }
    }
}

struct LastError {
    status: i32,
    native: i32,
    message: CString,
}
impl Default for LastError {
    fn default() -> Self {
        Self {
            status: OK,
            native: 0,
            message: CString::default(),
        }
    }
}
thread_local! {
    static LAST_ERROR: RefCell<LastError> = RefCell::new(LastError::default());
    static REGISTRY: RefCell<HashMap<u64, Resource>> = RefCell::new(HashMap::new());
}
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

pub enum Resource {
    Senders(Vec<SenderName>),
    Receiver(SpoutReceiver),
    Crop(crate::crop::CropState),
    Renderer(Box<crate::rendering::RenderState>),
}

pub fn api(call: impl FnOnce() -> Result<(), Error>) -> i32 {
    let result = match catch_unwind(AssertUnwindSafe(call)) {
        Ok(result) => result,
        Err(_) => Err(Error {
            status: PANIC,
            native: 0,
            message: "Rust panic was contained at the C ABI boundary; release affected handles"
                .into(),
        }),
    };
    LAST_ERROR
        .try_with(|slot| {
            let Ok(mut slot) = slot.try_borrow_mut() else {
                return PANIC;
            };
            match result {
                Ok(()) => {
                    // The ordinary render path performs no diagnostic allocation.
                    if slot.status != OK {
                        *slot = LastError::default();
                    }
                    OK
                }
                Err(error) => {
                    let status = error.status;
                    *slot = LastError {
                        status,
                        native: error.native,
                        message: CString::new(error.message.replace('\0', " "))
                            .expect("NUL removed"),
                    };
                    status
                }
            }
        })
        .unwrap_or(PANIC)
}
pub fn error_code() -> i32 {
    LAST_ERROR
        .try_with(|slot| slot.try_borrow().map_or(PANIC, |slot| slot.status))
        .unwrap_or(PANIC)
}
pub fn native_code() -> i32 {
    LAST_ERROR
        .try_with(|slot| slot.try_borrow().map_or(0, |slot| slot.native))
        .unwrap_or(0)
}
pub fn error_message() -> *const c_char {
    let unavailable = c"Thread-local diagnostics unavailable".as_ptr();
    LAST_ERROR
        .try_with(|slot| {
            slot.try_borrow()
                .map_or(unavailable, |slot| slot.message.as_ptr())
        })
        .unwrap_or(unavailable)
}

pub fn with_registry<T>(
    call: impl FnOnce(&mut HashMap<u64, Resource>) -> Result<T, Error>,
) -> Result<T, Error> {
    REGISTRY.with(|registry| {
        let mut registry = registry
            .try_borrow_mut()
            .map_err(|_| Error::argument("Reentrant handle access is unsupported"))?;
        call(&mut registry)
    })
}
pub fn insert(resource: Resource) -> Result<u64, Error> {
    let handle = NEXT_HANDLE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| Error::argument("Handle identifiers exhausted"))?;
    with_registry(|registry| {
        registry.insert(handle, resource);
        Ok(handle)
    })
}

/// Validate before every typed dereference. The caller still owns pointer validity.
pub fn pointer<T>(pointer: *const T) -> Result<(), Error> {
    if pointer.is_null() || !pointer.is_aligned() {
        Err(Error::argument("Null or misaligned pointer"))
    } else {
        Ok(())
    }
}
pub unsafe fn output<T>(out: *mut T, value: T) -> Result<(), Error> {
    pointer(out)?;
    unsafe { out.write(value) };
    Ok(())
}
pub unsafe fn sized_output<T>(out: *mut T, value: T) -> Result<(), Error> {
    pointer(out)?;
    if unsafe { out.cast::<u32>().read() } < size_of::<T>() as u32 {
        return Err(Error::argument(
            "struct_size is smaller than the ABI structure",
        ));
    }
    unsafe { out.write(value) };
    Ok(())
}
pub unsafe fn name(bytes: *const u8, length: usize) -> Result<SenderName, Error> {
    if !(1..=255).contains(&length) {
        return Err(Error::argument("Sender name must contain 1..255 bytes"));
    }
    pointer(bytes)?;
    Ok(SenderName::from_bytes(unsafe {
        std::slice::from_raw_parts(bytes, length)
    })?)
}
pub unsafe fn copy_bytes(
    bytes: &[u8],
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
    nul: bool,
) -> Result<(), Error> {
    pointer(required)?;
    if capacity > isize::MAX as usize || (buffer.is_null() && capacity != 0) {
        return Err(Error::argument("Invalid buffer capacity or pointer"));
    }
    let length = bytes.len() + usize::from(nul);
    unsafe { required.write(length) };
    if capacity < length {
        return Err(Error {
            status: BUFFER_TOO_SMALL,
            native: 0,
            message: "Output buffer too small; see required byte count".into(),
        });
    }
    if length > 0 {
        pointer(buffer)?;
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
            if nul {
                buffer.add(bytes.len()).write(0);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panic_is_contained_and_error_queries_preserve_it() {
        assert_eq!(api(|| panic!("boundary probe")), PANIC);
        assert_eq!(error_code(), PANIC);
        assert!(!error_message().is_null());
        assert_eq!(error_code(), PANIC);
        assert_eq!(api(|| Ok(())), OK);
        assert_eq!(error_code(), OK);
    }
    #[test]
    fn native_hresult_survives_context_errors() {
        let error = Error::gpu(spout_transparent::AppError::context(
            "prepare",
            spout_transparent::windows::core::Error::from_hresult(
                spout_transparent::windows::core::HRESULT(0x80004005u32 as i32),
            ),
        ));
        assert_eq!(error.native, 0x80004005u32 as i32);
    }
}
