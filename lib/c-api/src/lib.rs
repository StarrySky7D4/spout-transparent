//! Windows x64 C ABI. See include/spout_transparent.h for the complete contract.
#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
compile_error!("The C ABI currently supports Windows x64 only");
mod boundary;
mod crop;
mod receiver;
mod rendering;
mod types;

use boundary::{api, with_registry, Error};
use std::ffi::c_char;
pub use types::*;

/// ABI major version. This query preserves the calling thread's last error.
#[no_mangle]
pub extern "C" fn spt_abi_version() -> u32 {
    1
}

/// Thread-local error code. This query does not clear the error.
#[no_mangle]
pub extern "C" fn spt_last_error_code() -> i32 {
    boundary::error_code()
}

/// Native Win32 code or HRESULT, or zero when unavailable. Preserves last error.
#[no_mangle]
pub extern "C" fn spt_last_native_error() -> i32 {
    boundary::native_code()
}

/// Borrowed UTF-8 text, valid until the next ordinary API call on this thread.
/// Never free or write this pointer; copy it before another API call.
#[no_mangle]
pub extern "C" fn spt_last_error_message() -> *const c_char {
    boundary::error_message()
}

/// Release any handle on its creating thread. Zero is a no-op; stale handles fail.
#[no_mangle]
pub extern "C" fn spt_release(handle: u64) -> i32 {
    api(|| {
        if handle == 0 {
            return Ok(());
        }
        // Remove first, then drop outside the registry borrow.
        let resource =
            with_registry(|registry| registry.remove(&handle).ok_or_else(Error::handle))?;
        drop(resource);
        Ok(())
    })
}

fn wrong_resource<T>() -> Result<T, Error> {
    Err(Error::handle())
}
