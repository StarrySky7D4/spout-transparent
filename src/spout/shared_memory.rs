//! RAII ownership for Spout mappings, mapped views and named access mutexes.
use super::SenderName;
use std::{
    ffi::{c_void, CString},
    io,
    mem::{size_of, zeroed},
    ptr::{self, NonNull},
};
use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingA, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ,
    MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS,
};
use windows_sys::Win32::System::Threading::{CreateMutexA, ReleaseMutex, WaitForSingleObject};

pub(super) struct SharedMap {
    // Unmap before releasing the mapping handle.
    view: MappedView,
    _mapping: OwnedHandle,
    len: usize,
    mutex: NamedMutex,
}

impl SharedMap {
    pub(super) fn open(name: &[u8]) -> io::Result<Self> {
        let map_name = CString::new(name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "map name contains NUL"))?;
        let mapping = OwnedHandle::new(unsafe {
            OpenFileMappingA(FILE_MAP_READ, 0, map_name.as_ptr().cast())
        })
        .map_err(normalize_not_found)?;
        let mapped = unsafe { MapViewOfFile(mapping.0, FILE_MAP_READ, 0, 0, 0) };
        let view = MappedView(NonNull::new(mapped.Value).ok_or_else(io::Error::last_os_error)?);

        let mut memory: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        let queried = unsafe {
            VirtualQuery(
                view.0.as_ptr().cast_const(),
                &mut memory,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried == 0 || memory.RegionSize == 0 {
            return Err(io::Error::last_os_error());
        }
        let mutex = NamedMutex::with_suffix(name, b"_mutex")?;

        Ok(Self {
            _mapping: mapping,
            view,
            len: memory.RegionSize,
            mutex,
        })
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn read(&self, requested: usize, timeout_ms: u32) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; requested.min(self.len)];
        self.read_into(&mut bytes, timeout_ms)?;
        Ok(bytes)
    }

    pub(super) fn read_into(&self, bytes: &mut [u8], timeout_ms: u32) -> io::Result<usize> {
        let _guard = self
            .mutex
            .lock(timeout_ms)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "Spout metadata is busy"))?;
        let len = bytes.len().min(self.len);
        unsafe {
            ptr::copy_nonoverlapping(self.view.0.as_ptr().cast::<u8>(), bytes.as_mut_ptr(), len);
        }
        Ok(len)
    }
}

/// Non-null kernel handle. Every successful acquisition has one close path,
/// including failures after a mapping or semaphore has already been opened.
pub(super) struct OwnedHandle(pub(super) HANDLE);
impl OwnedHandle {
    pub(super) fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct MappedView(NonNull<c_void>);
impl Drop for MappedView {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.0.as_ptr(),
            });
        }
    }
}

pub struct NamedMutex {
    handle: OwnedHandle,
}

impl NamedMutex {
    pub fn for_sender_texture(sender: &SenderName) -> io::Result<Self> {
        Self::with_suffix(&sender.0, b"_SpoutAccessMutex")
    }

    fn with_suffix(name: &[u8], suffix: &[u8]) -> io::Result<Self> {
        let mut mutex_name = Vec::with_capacity(name.len() + suffix.len());
        mutex_name.extend_from_slice(name);
        mutex_name.extend_from_slice(suffix);
        let mutex_name = CString::new(mutex_name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "mutex name contains NUL"))?;
        let handle = unsafe { CreateMutexA(ptr::null(), 0, mutex_name.as_ptr().cast()) };
        Ok(Self {
            handle: OwnedHandle::new(handle)?,
        })
    }

    pub fn try_lock(&self) -> io::Result<Option<NamedMutexGuard<'_>>> {
        self.lock(0)
    }

    fn lock(&self, timeout_ms: u32) -> io::Result<Option<NamedMutexGuard<'_>>> {
        match unsafe { WaitForSingleObject(self.handle.0, timeout_ms) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(NamedMutexGuard { mutex: self })),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

pub struct NamedMutexGuard<'a> {
    mutex: &'a NamedMutex,
}

impl Drop for NamedMutexGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.mutex.handle.0);
        }
    }
}

fn normalize_not_found(error: io::Error) -> io::Error {
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) {
        io::Error::new(io::ErrorKind::NotFound, error)
    } else {
        error
    }
}
