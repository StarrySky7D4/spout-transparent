//! Background discovery publishes changed snapshots through a caller-provided wake callback.
use super::{
    protocol::SENDER_NAME_CAPACITY, read_sender_info, shared_memory::SharedMap, SenderName,
};
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

/// Read the registered sender names. An absent registry is an empty list.
///
/// This call may wait up to 67 ms for the registry mutex. Use [`SenderDiscovery`]
/// when discovery must not block a UI or render thread.
pub fn discover_sender_names() -> io::Result<Vec<SenderName>> {
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
    /// Start discovery at 500 ms intervals, notifying only when names or error kind change.
    ///
    /// `notify` runs on the discovery worker, after the snapshot is available through
    /// [`Self::take_latest`]. Return `false` to stop, for example if a channel or
    /// event-loop proxy has disconnected. The callback must return promptly and must
    /// not drop this worker from inside itself (Drop joins the worker thread).
    pub fn start(notify: impl FnMut() -> bool + Send + 'static) -> io::Result<Self> {
        Self::start_with_query(notify, discover_sender_names)
    }

    fn start_with_query(
        mut notify: impl FnMut() -> bool + Send + 'static,
        mut query: impl FnMut() -> DiscoveryResult + Send + 'static,
    ) -> io::Result<Self> {
        let latest = Arc::new(Mutex::new(None));
        let worker_latest = Arc::clone(&latest);
        let (stop_tx, stop_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("spout-discovery".into())
            .spawn(move || {
                let mut previous = None;
                loop {
                    let result = query();
                    let key = result
                        .as_ref()
                        .map(Clone::clone)
                        .map_err(|error| error.kind());
                    if previous.as_ref() != Some(&key) {
                        previous = Some(key);
                        *worker_latest
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
                        if !notify() {
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

    /// Consume the latest snapshot. Intermediate snapshots may be replaced.
    pub fn take_latest(&self) -> Option<io::Result<Vec<SenderName>>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_can_stop_worker_after_snapshot_is_published() {
        let (sent, received) = mpsc::channel();
        let sender = SenderName::from_bytes("test source").unwrap();
        let discovered = sender.clone();
        let discovery = SenderDiscovery::start_with_query(
            move || {
                sent.send(()).unwrap();
                false
            },
            move || Ok(vec![discovered.clone()]),
        )
        .unwrap();
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(discovery.take_latest().unwrap().unwrap(), vec![sender]);
        assert!(discovery.take_latest().is_none());
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }
}
