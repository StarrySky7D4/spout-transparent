//! Background discovery publishes only changed snapshots through the event proxy.
use super::{
    protocol::SENDER_NAME_CAPACITY, read_sender_info, shared_memory::SharedMap, SenderName,
};
use crate::events::{AppEvent, EventSender};
use std::{
    io,
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

const SENDER_NAMES_MAP: &[u8] = b"SpoutSenderNames";
const MAX_SENDER_LIST_BYTES: usize = SENDER_NAME_CAPACITY * 4096;
const METADATA_LOCK_TIMEOUT_MS: u32 = 67;
const REALTIME_METADATA_LOCK_TIMEOUT_MS: u32 = 0;
const SENDER_DISCOVERY_INTERVAL: Duration = Duration::from_millis(500);

pub(crate) fn discover_sender_names() -> io::Result<Vec<SenderName>> {
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
    pub fn start(events: EventSender) -> io::Result<Self> {
        let latest = Arc::new(Mutex::new(None));
        let worker_latest = Arc::clone(&latest);
        let (stop_tx, stop_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("spout-discovery".into())
            .spawn(move || {
                let mut previous = None;
                loop {
                    let result = discover_sender_names();
                    let key = result
                        .as_ref()
                        .map(Clone::clone)
                        .map_err(|error| error.kind());
                    if previous.as_ref() != Some(&key) {
                        previous = Some(key);
                        *worker_latest
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
                        if events.send_event(AppEvent::DiscoveryChanged).is_err() {
                            break;
                        }
                    }

                    match stop_rx.recv_timeout(SENDER_DISCOVERY_INTERVAL) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
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
