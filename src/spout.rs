use std::ffi::{c_void, CString};
use std::fmt;
use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingA, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ,
    MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS,
};
use windows_sys::Win32::System::Threading::{CreateMutexA, ReleaseMutex, WaitForSingleObject};

const SENDER_NAMES_MAP: &[u8] = b"SpoutSenderNames";
const SENDER_NAME_CAPACITY: usize = 256;
const MAX_SENDER_LIST_BYTES: usize = SENDER_NAME_CAPACITY * 4096;
const SHARED_TEXTURE_INFO_SIZE: usize = 280;
const METADATA_LOCK_TIMEOUT_MS: u32 = 67;

#[derive(Clone, Eq, PartialEq)]
pub struct SenderName(Vec<u8>);

impl SenderName {
    fn from_slot(slot: &[u8]) -> Option<Self> {
        let end = slot
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(slot.len());
        (end > 0).then(|| Self(slot[..end].to_vec()))
    }

    pub fn display_name(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }
}

impl fmt::Debug for SenderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SenderName")
            .field(&self.display_name())
            .finish()
    }
}

impl fmt::Display for SenderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display_name())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderInfo {
    pub share_handle: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub usage: u32,
    pub description: String,
    pub partner_id: u32,
}

impl SenderInfo {
    fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() < SHARED_TEXTURE_INFO_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Spout sender metadata is truncated",
            ));
        }

        let read_u32 = |offset: usize| {
            u32::from_le_bytes(
                bytes[offset..offset + 4]
                    .try_into()
                    .expect("checked length"),
            )
        };
        let description_bytes = &bytes[20..276];
        let description_end = description_bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(description_bytes.len());

        Ok(Self {
            share_handle: read_u32(0),
            width: read_u32(4),
            height: read_u32(8),
            format: read_u32(12),
            usage: read_u32(16),
            description: String::from_utf8_lossy(&description_bytes[..description_end])
                .into_owned(),
            partner_id: read_u32(276),
        })
    }

    fn is_usable(&self) -> bool {
        self.share_handle != 0 && self.share_handle != u32::MAX && self.width > 0 && self.height > 0
    }

    pub fn raw_handle(&self) -> *mut c_void {
        // Spout stores a Win32 HANDLE in a 32-bit LONG even in x64 builds.
        self.share_handle as i32 as isize as *mut c_void
    }
}

pub struct SpoutReceiver {
    selected: Option<SenderName>,
    current: Option<SenderInfo>,
    generation: u64,
    next_discovery: Instant,
}

impl SpoutReceiver {
    pub fn new() -> Self {
        Self {
            selected: None,
            current: None,
            generation: 0,
            next_discovery: Instant::now(),
        }
    }

    pub fn sender_names(&self) -> io::Result<Vec<SenderName>> {
        let map = match SharedMap::open(SENDER_NAMES_MAP) {
            Ok(map) => map,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let bytes = map.read(map.len().min(MAX_SENDER_LIST_BYTES))?;
        let mut names = Vec::new();

        for slot in bytes.chunks_exact(SENDER_NAME_CAPACITY) {
            let Some(name) = SenderName::from_slot(slot) else {
                break;
            };
            // Stale entries can survive an unclean sender shutdown. Match Spout's
            // behavior by returning only names with a live metadata mapping.
            if read_sender_info(&name).is_ok() {
                names.push(name);
            }
        }
        Ok(names)
    }

    pub fn select(&mut self, sender: SenderName) {
        self.selected = Some(sender);
        self.current = None;
        self.next_discovery = Instant::now();
    }

    pub fn poll(&mut self) -> io::Result<bool> {
        let result = self.selected.as_ref().map(read_sender_info);
        match result {
            None => self.discover_sender(),
            Some(Ok(info)) if info.is_usable() => {
                if self
                    .current
                    .as_ref()
                    .is_none_or(|current| resource_changed(current, &info))
                {
                    self.generation = self.generation.wrapping_add(1);
                }
                self.current = Some(info);
                Ok(true)
            }
            Some(Ok(_)) => {
                self.current = None;
                Ok(false)
            }
            Some(Err(error)) if error.kind() == io::ErrorKind::NotFound => {
                self.current = None;
                self.discover_sender()
            }
            Some(Err(error)) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Some(Err(error)) => Err(error),
        }
    }

    fn discover_sender(&mut self) -> io::Result<bool> {
        if Instant::now() < self.next_discovery {
            return Ok(false);
        }
        self.next_discovery = Instant::now() + Duration::from_millis(500);

        let Some(name) = self.sender_names()?.into_iter().next() else {
            return Ok(false);
        };
        let info = read_sender_info(&name)?;
        if !info.is_usable() {
            return Ok(false);
        }

        self.selected = Some(name);
        self.current = Some(info);
        self.generation = self.generation.wrapping_add(1);
        Ok(true)
    }

    pub fn current_name(&self) -> Option<&SenderName> {
        self.selected.as_ref()
    }

    pub fn sender_handle(&self) -> *mut c_void {
        self.current
            .as_ref()
            .map(SenderInfo::raw_handle)
            .unwrap_or(ptr::null_mut())
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

fn resource_changed(previous: &SenderInfo, current: &SenderInfo) -> bool {
    previous.share_handle != current.share_handle
        || previous.width != current.width
        || previous.height != current.height
        || previous.format != current.format
        || previous.usage != current.usage
        || previous.partner_id != current.partner_id
}

fn read_sender_info(sender: &SenderName) -> io::Result<SenderInfo> {
    let map = SharedMap::open(&sender.0)?;
    SenderInfo::parse(&map.read(SHARED_TEXTURE_INFO_SIZE)?)
}

struct SharedMap {
    mapping: HANDLE,
    view: NonNull<c_void>,
    len: usize,
    mutex: NamedMutex,
}

impl SharedMap {
    fn open(name: &[u8]) -> io::Result<Self> {
        let map_name = CString::new(name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "map name contains NUL"))?;
        let mapping = unsafe { OpenFileMappingA(FILE_MAP_READ, 0, map_name.as_ptr().cast()) };
        if mapping.is_null() {
            return Err(normalize_not_found(io::Error::last_os_error()));
        }

        let mapped = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) };
        let Some(view) = NonNull::new(mapped.Value) else {
            unsafe {
                CloseHandle(mapping);
            }
            return Err(io::Error::last_os_error());
        };

        let mut memory: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        let queried = unsafe {
            VirtualQuery(
                view.as_ptr().cast_const(),
                &mut memory,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried == 0 || memory.RegionSize == 0 {
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: view.as_ptr(),
                });
                CloseHandle(mapping);
            }
            return Err(io::Error::last_os_error());
        }

        let mutex = match NamedMutex::with_suffix(name, b"_mutex") {
            Ok(mutex) => mutex,
            Err(error) => {
                unsafe {
                    UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: view.as_ptr(),
                    });
                    CloseHandle(mapping);
                }
                return Err(error);
            }
        };

        Ok(Self {
            mapping,
            view,
            len: memory.RegionSize,
            mutex,
        })
    }

    fn len(&self) -> usize {
        self.len
    }

    fn read(&self, requested: usize) -> io::Result<Vec<u8>> {
        let _guard = self
            .mutex
            .lock(METADATA_LOCK_TIMEOUT_MS)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "Spout metadata is busy"))?;
        let len = requested.min(self.len);
        let mut bytes = vec![0; len];
        unsafe {
            ptr::copy_nonoverlapping(self.view.as_ptr().cast::<u8>(), bytes.as_mut_ptr(), len);
        }
        Ok(bytes)
    }
}

impl Drop for SharedMap {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.as_ptr(),
            });
            CloseHandle(self.mapping);
        }
    }
}

pub struct NamedMutex {
    handle: HANDLE,
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
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self { handle })
        }
    }

    pub fn try_lock(&self) -> io::Result<Option<NamedMutexGuard<'_>>> {
        self.lock(0)
    }

    fn lock(&self, timeout_ms: u32) -> io::Result<Option<NamedMutexGuard<'_>>> {
        match unsafe { WaitForSingleObject(self.handle, timeout_ms) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(NamedMutexGuard { mutex: self })),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

pub struct NamedMutexGuard<'a> {
    mutex: &'a NamedMutex,
}

impl Drop for NamedMutexGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.mutex.handle);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sender_names_until_empty_slot() {
        let mut bytes = vec![0; SENDER_NAME_CAPACITY * 3];
        bytes[..5].copy_from_slice(b"Alpha");
        bytes[SENDER_NAME_CAPACITY..SENDER_NAME_CAPACITY + 4].copy_from_slice(b"Beta");

        let names: Vec<_> = bytes
            .chunks_exact(SENDER_NAME_CAPACITY)
            .map_while(SenderName::from_slot)
            .map(|name| name.display_name())
            .collect();
        assert_eq!(names, ["Alpha", "Beta"]);
    }

    #[test]
    fn parses_spout_shared_texture_info_layout() {
        let mut bytes = [0u8; SHARED_TEXTURE_INFO_SIZE];
        bytes[0..4].copy_from_slice(&0x89ab_cdefu32.to_le_bytes());
        bytes[4..8].copy_from_slice(&1920u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&1080u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&87u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
        bytes[20..25].copy_from_slice(b"alpha");
        bytes[276..280].copy_from_slice(&42u32.to_le_bytes());

        let info = SenderInfo::parse(&bytes).unwrap();
        assert_eq!(info.share_handle, 0x89ab_cdef);
        assert_eq!((info.width, info.height), (1920, 1080));
        assert_eq!(info.format, 87);
        assert_eq!(info.usage, 1);
        assert_eq!(info.description, "alpha");
        assert_eq!(info.partner_id, 42);
    }

    #[test]
    fn sign_extends_spout_handle_on_64_bit_targets() {
        let info = SenderInfo {
            share_handle: 0x89ab_cdef,
            width: 1,
            height: 1,
            format: 0,
            usage: 0,
            description: String::new(),
            partner_id: 0,
        };
        assert_eq!(info.raw_handle() as isize, 0x89ab_cdefu32 as i32 as isize);
    }

    #[test]
    fn detects_recreated_resource_even_when_handle_is_reused() {
        let previous = SenderInfo {
            share_handle: 7,
            width: 1920,
            height: 1080,
            format: 87,
            usage: 0,
            description: String::new(),
            partner_id: 10,
        };
        let current = SenderInfo {
            partner_id: 11,
            ..previous.clone()
        };

        assert!(resource_changed(&previous, &current));
    }
}
