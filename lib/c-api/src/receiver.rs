use crate::{
    boundary::{self, api, insert, with_registry, Error, Resource},
    types::*,
    wrong_resource,
};
use spout_transparent::{discover_sender_names, read_sender_info, SpoutReceiver};
use std::time::Instant;

/// Snapshot the registered sender list without changing the global registry.
/// # Safety
/// `out` must point to writable, aligned u64 storage.
#[no_mangle]
pub unsafe extern "C" fn spt_sender_list_create(out: *mut u64) -> i32 {
    api(|| {
        unsafe { boundary::output(out, 0)? };
        let handle = insert(Resource::Senders(discover_sender_names()?))?;
        unsafe { boundary::output(out, handle) }
    })
}

/// Read snapshot length.
/// # Safety
/// `out` must point to writable, aligned size_t storage.
#[no_mangle]
pub unsafe extern "C" fn spt_sender_list_count(handle: u64, out: *mut usize) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get(&handle) {
            Some(Resource::Senders(names)) => unsafe { boundary::output(out, names.len()) },
            _ => wrong_resource(),
        })
    })
}

/// Copy a raw name plus a terminating NUL; required includes that NUL.
/// # Safety
/// `required` is writable size_t storage; non-null buffer addresses capacity writable bytes.
#[no_mangle]
pub unsafe extern "C" fn spt_sender_list_name(
    handle: u64,
    index: usize,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get(&handle) {
            Some(Resource::Senders(names)) => {
                let name = names
                    .get(index)
                    .ok_or_else(|| Error::argument("Sender index out of range"))?;
                unsafe { boundary::copy_bytes(name.as_bytes(), buffer, capacity, required, true) }
            }
            _ => wrong_resource(),
        })
    })
}

/// Read metadata by raw name; generation is zero for this stateless query.
/// # Safety
/// Name addresses length readable bytes; info is writable and its struct_size is initialized.
#[no_mangle]
pub unsafe extern "C" fn spt_sender_info_read(
    name: *const u8,
    length: usize,
    timeout_ms: u32,
    info: *mut SptSenderInfo,
) -> i32 {
    api(|| {
        let name = unsafe { boundary::name(name, length)? };
        let metadata = read_sender_info(&name, timeout_ms)?;
        unsafe { boundary::sized_output(info, SptSenderInfo::new(&metadata, 0)) }
    })
}

/// Create a disconnected receiver. Call refresh for discovery or select a raw name.
/// # Safety
/// Out is writable, aligned u64 storage.
#[no_mangle]
pub unsafe extern "C" fn spt_receiver_create(out: *mut u64) -> i32 {
    api(|| {
        unsafe { boundary::output(out, 0)? };
        let handle = insert(Resource::Receiver(SpoutReceiver::new()))?;
        unsafe { boundary::output(out, handle) }
    })
}

/// Update discovery. This may wait on the global registry; do not call every frame.
#[no_mangle]
pub extern "C" fn spt_receiver_refresh(handle: u64) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Receiver(receiver)) => {
                receiver.update_discovered(discover_sender_names()?);
                Ok(())
            }
            _ => wrong_resource(),
        })
    })
}

/// Select a source, which need not appear in the global sender list.
/// # Safety
/// Name addresses length readable bytes, without a terminating NUL in length.
#[no_mangle]
pub unsafe extern "C" fn spt_receiver_select(handle: u64, name: *const u8, length: usize) -> i32 {
    api(|| {
        let name = unsafe { boundary::name(name, length)? };
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Receiver(receiver)) => {
                receiver.select(name);
                Ok(())
            }
            _ => wrong_resource(),
        })
    })
}

/// Reset selection, cache and discovery. Call refresh again for automatic selection.
#[no_mangle]
pub extern "C" fn spt_receiver_reset(handle: u64) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Receiver(receiver)) => {
                *receiver = SpoutReceiver::new();
                Ok(())
            }
            _ => wrong_resource(),
        })
    })
}

/// Nonblocking metadata poll. A disconnected source is OK with connected=0.
/// # Safety
/// Connected points to writable, aligned u32 storage.
#[no_mangle]
pub unsafe extern "C" fn spt_receiver_poll(handle: u64, connected: *mut u32) -> i32 {
    api(|| {
        boundary::pointer(connected)?;
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Receiver(receiver)) => unsafe {
                boundary::output(connected, u32::from(receiver.poll()?))
            },
            _ => wrong_resource(),
        })
    })
}

/// Read cached metadata and resource generation, without accessing shared memory.
/// # Safety
/// Info is writable aligned storage with struct_size initialized.
#[no_mangle]
pub unsafe extern "C" fn spt_receiver_info(handle: u64, info: *mut SptSenderInfo) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get(&handle) {
            Some(Resource::Receiver(receiver)) => {
                let metadata = receiver
                    .sender_info()
                    .ok_or_else(|| Error::not_found("Receiver disconnected"))?;
                let identity = receiver
                    .identity()
                    .ok_or_else(|| Error::not_found("Receiver disconnected"))?;
                unsafe {
                    boundary::sized_output(info, SptSenderInfo::new(metadata, identity.generation))
                }
            }
            _ => wrong_resource(),
        })
    })
}

/// Copy the connected name as raw bytes plus NUL.
/// # Safety
/// Required is writable; non-null buffer addresses capacity writable bytes.
#[no_mangle]
pub unsafe extern "C" fn spt_receiver_name(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get(&handle) {
            Some(Resource::Receiver(receiver)) => {
                let identity = receiver
                    .identity()
                    .ok_or_else(|| Error::not_found("Receiver disconnected"))?;
                unsafe {
                    boundary::copy_bytes(identity.name.as_bytes(), buffer, capacity, required, true)
                }
            }
            _ => wrong_resource(),
        })
    })
}

/// Copy the connected description as UTF-8 plus NUL.
/// # Safety
/// Required is writable; non-null buffer addresses capacity writable bytes.
#[no_mangle]
pub unsafe extern "C" fn spt_receiver_description(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get(&handle) {
            Some(Resource::Receiver(receiver)) => {
                let metadata = receiver
                    .sender_info()
                    .ok_or_else(|| Error::not_found("Receiver disconnected"))?;
                unsafe {
                    boundary::copy_bytes(
                        metadata.description.as_bytes(),
                        buffer,
                        capacity,
                        required,
                        true,
                    )
                }
            }
            _ => wrong_resource(),
        })
    })
}

/// Milliseconds until the next metadata poll, rounded up; UINT64_MAX means none.
/// # Safety
/// Delay is writable, aligned u64 storage.
#[no_mangle]
pub unsafe extern "C" fn spt_receiver_next_poll_ms(handle: u64, delay: *mut u64) -> i32 {
    api(|| {
        with_registry(|registry| match registry.get(&handle) {
            Some(Resource::Receiver(receiver)) => unsafe {
                boundary::output(delay, receiver.next_poll().map_or(u64::MAX, delay_ms))
            },
            _ => wrong_resource(),
        })
    })
}

pub fn delay_ms(deadline: Instant) -> u64 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_nanos()
        .div_ceil(1_000_000)
        .min(u64::MAX as u128) as u64
}
