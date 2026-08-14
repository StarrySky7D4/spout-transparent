use std::ffi::{c_void, CString};
use std::fmt;
use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::{self, NonNull};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingA, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ,
    MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexA, CreateSemaphoreA, ReleaseMutex, ReleaseSemaphore, WaitForSingleObject,
};

const SENDER_NAMES_MAP: &[u8] = b"SpoutSenderNames";
const SENDER_NAME_CAPACITY: usize = 256;
const MAX_SENDER_LIST_BYTES: usize = SENDER_NAME_CAPACITY * 4096;
const SHARED_TEXTURE_INFO_SIZE: usize = 280;
const METADATA_LOCK_TIMEOUT_MS: u32 = 67;
const REALTIME_METADATA_LOCK_TIMEOUT_MS: u32 = 0;
const CONNECTED_METADATA_POLL_INTERVAL: Duration = Duration::from_millis(100);
const DISCONNECTED_METADATA_POLL_INTERVAL: Duration = Duration::from_millis(500);
const SENDER_DISCOVERY_INTERVAL: Duration = Duration::from_millis(500);

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

    #[cfg(test)]
    pub(crate) fn for_test(name: &str) -> Self {
        Self(name.as_bytes().to_vec())
    }

    fn with_suffix(&self, suffix: &[u8]) -> io::Result<CString> {
        let mut name = Vec::with_capacity(self.0.len() + suffix.len());
        name.extend_from_slice(&self.0);
        name.extend_from_slice(suffix);
        CString::new(name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "sender name contains NUL"))
    }
}

pub struct FrameCounter {
    handle: HANDLE,
    last_count: i32,
}

impl FrameCounter {
    pub fn for_sender(sender: &SenderName) -> io::Result<Self> {
        let name = sender.with_suffix(b"_Count_Semaphore")?;
        // Spout deliberately lets either endpoint create the semaphore. A count
        // of zero after the probe means the sender is not using frame counting.
        let handle = unsafe { CreateSemaphoreA(ptr::null(), 1, i32::MAX, name.as_ptr().cast()) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self {
                handle,
                last_count: 0,
            })
        }
    }

    pub fn is_new_frame(&mut self) -> bool {
        match unsafe { WaitForSingleObject(self.handle, 0) } {
            WAIT_OBJECT_0 => {
                let mut count_after_wait = 0;
                if unsafe { ReleaseSemaphore(self.handle, 1, &mut count_after_wait) } == 0 {
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

impl Drop for FrameCounter {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
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
    discovered: Vec<SenderName>,
    generation: u64,
    next_metadata_poll: Instant,
    next_discovery: Instant,
}

impl SpoutReceiver {
    pub fn new() -> Self {
        Self {
            selected: None,
            current: None,
            discovered: Vec::new(),
            generation: 0,
            next_metadata_poll: Instant::now(),
            next_discovery: Instant::now(),
        }
    }

    pub fn update_discovered(&mut self, senders: Vec<SenderName>) {
        self.discovered = senders;
    }

    pub fn select(&mut self, sender: SenderName) {
        self.selected = Some(sender);
        self.current = None;
        self.next_metadata_poll = Instant::now();
        self.next_discovery = Instant::now();
    }

    pub fn poll(&mut self) -> io::Result<bool> {
        let now = Instant::now();
        if now < self.next_metadata_poll {
            return Ok(self.current.is_some());
        }
        self.schedule_metadata_poll(now, self.current.is_some());

        let result = self
            .selected
            .as_ref()
            .map(|sender| read_sender_info(sender, REALTIME_METADATA_LOCK_TIMEOUT_MS));
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
                self.schedule_metadata_poll(now, true);
                Ok(true)
            }
            Some(Ok(_)) => {
                self.current = None;
                self.schedule_metadata_poll(now, false);
                Ok(false)
            }
            Some(Err(error)) if error.kind() == io::ErrorKind::NotFound => {
                self.current = None;
                let connected = self.discover_sender()?;
                self.schedule_metadata_poll(now, connected);
                Ok(connected)
            }
            Some(Err(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                Ok(self.current.is_some())
            }
            Some(Err(error)) => Err(error),
        }
    }

    fn schedule_metadata_poll(&mut self, now: Instant, connected: bool) {
        self.next_metadata_poll = now + metadata_poll_interval(connected);
    }

    fn discover_sender(&mut self) -> io::Result<bool> {
        if Instant::now() < self.next_discovery {
            return Ok(false);
        }
        self.next_discovery = Instant::now() + Duration::from_millis(500);

        for name in self.discovered.clone() {
            match read_sender_info(&name, REALTIME_METADATA_LOCK_TIMEOUT_MS) {
                Ok(info) if info.is_usable() => {
                    self.selected = Some(name);
                    self.current = Some(info);
                    self.generation = self.generation.wrapping_add(1);
                    self.schedule_metadata_poll(Instant::now(), true);
                    return Ok(true);
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::WouldBlock
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(false)
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

fn metadata_poll_interval(connected: bool) -> Duration {
    if connected {
        CONNECTED_METADATA_POLL_INTERVAL
    } else {
        DISCONNECTED_METADATA_POLL_INTERVAL
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

fn read_sender_info(sender: &SenderName, timeout_ms: u32) -> io::Result<SenderInfo> {
    let map = SharedMap::open(&sender.0)?;
    SenderInfo::parse(&map.read(SHARED_TEXTURE_INFO_SIZE, timeout_ms)?)
}

fn discover_sender_names() -> io::Result<Vec<SenderName>> {
    let map = match SharedMap::open(SENDER_NAMES_MAP) {
        Ok(map) => map,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let bytes = map.read(
        map.len().min(MAX_SENDER_LIST_BYTES),
        METADATA_LOCK_TIMEOUT_MS,
    )?;
    let mut names = Vec::new();

    for slot in bytes.chunks_exact(SENDER_NAME_CAPACITY) {
        let Some(name) = SenderName::from_slot(slot) else {
            break;
        };
        // A busy metadata map is still live. Only mappings that are definitely
        // gone are filtered from the discovery snapshot.
        match read_sender_info(&name, REALTIME_METADATA_LOCK_TIMEOUT_MS) {
            Ok(_) => names.push(name),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => names.push(name),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => log::debug!("Could not validate Spout sender '{name}': {error}"),
        }
    }
    Ok(names)
}

type DiscoveryResult = io::Result<Vec<SenderName>>;

pub struct SenderDiscovery {
    latest: Arc<Mutex<Option<DiscoveryResult>>>,
    stop_tx: mpsc::SyncSender<()>,
    worker: Option<JoinHandle<()>>,
}

impl SenderDiscovery {
    pub fn start() -> io::Result<Self> {
        let latest = Arc::new(Mutex::new(None));
        let worker_latest = Arc::clone(&latest);
        let (stop_tx, stop_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("spout-discovery".into())
            .spawn(move || loop {
                let result = discover_sender_names();
                *worker_latest
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);

                match stop_rx.recv_timeout(SENDER_DISCOVERY_INTERVAL) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            })?;

        Ok(Self {
            latest,
            stop_tx,
            worker: Some(worker),
        })
    }

    pub fn take_latest(&self) -> Option<DiscoveryResult> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

impl Drop for SenderDiscovery {
    fn drop(&mut self) {
        let _ = self.stop_tx.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
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

    fn read(&self, requested: usize, timeout_ms: u32) -> io::Result<Vec<u8>> {
        let _guard = self
            .mutex
            .lock(timeout_ms)?
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

    #[test]
    fn disconnected_metadata_polling_is_less_frequent() {
        assert!(metadata_poll_interval(false) > metadata_poll_interval(true));
        assert_eq!(
            metadata_poll_interval(true),
            CONNECTED_METADATA_POLL_INTERVAL
        );
    }

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
