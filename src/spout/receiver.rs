//! UI-thread metadata state; generation changes invalidate installed GPU resources.
use super::{read_sender_info, SenderIdentity, SenderInfo, SenderName};
use std::{
    io,
    time::{Duration, Instant},
};

const REALTIME_METADATA_LOCK_TIMEOUT_MS: u32 = 0;
const CONNECTED_METADATA_POLL_INTERVAL: Duration = Duration::from_millis(100);
const DISCONNECTED_METADATA_POLL_INTERVAL: Duration = Duration::from_millis(500);

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
        if self.discovered == senders {
            return;
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|name| !senders.contains(name))
        {
            self.current = None;
            self.selected = None;
        }
        self.discovered = senders;
        if self.current.is_none() {
            self.next_metadata_poll = Instant::now();
            self.next_discovery = Instant::now();
        }
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
                if self.discovered.is_empty() {
                    self.selected = None;
                }
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

        for name in &self.discovered {
            match read_sender_info(name, REALTIME_METADATA_LOCK_TIMEOUT_MS) {
                Ok(info) if info.is_usable() => {
                    self.selected = Some(name.clone());
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

    pub fn identity(&self) -> Option<SenderIdentity> {
        Some(SenderIdentity {
            name: self.selected.clone()?,
            share_handle: self.current.as_ref()?.share_handle,
            generation: self.generation,
        })
    }

    pub fn next_poll(&self) -> Option<Instant> {
        (self.selected.is_some() || !self.discovered.is_empty()).then_some(self.next_metadata_poll)
    }

    pub fn matches(&self, identity: &SenderIdentity) -> bool {
        self.selected.as_ref() == Some(&identity.name)
            && self.generation == identity.generation
            && self
                .current
                .as_ref()
                .is_some_and(|info| info.share_handle == identity.share_handle)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removal_of_the_discovered_source_invalidates_identity_and_ui_polling() {
        let name = SenderName::for_test("source");
        let mut receiver = SpoutReceiver::new();
        receiver.discovered = vec![name.clone()];
        receiver.selected = Some(name);
        receiver.current = Some(SenderInfo {
            share_handle: 7,
            width: 16,
            height: 16,
            format: 87,
            usage: 0,
            description: String::new(),
            partner_id: 1,
        });
        receiver.generation = 1;
        let old = receiver.identity().unwrap();
        assert!(receiver.matches(&old));
        receiver.update_discovered(Vec::new());
        assert_eq!(receiver.identity(), None);
        assert_eq!(receiver.next_poll(), None);
        assert!(!receiver.matches(&old));
    }

    #[test]
    fn a_new_discovery_cancels_the_disconnected_poll_delay() {
        let mut receiver = SpoutReceiver::new();
        receiver.next_metadata_poll = Instant::now() + Duration::from_millis(500);
        receiver.next_discovery = receiver.next_metadata_poll;
        receiver.update_discovered(vec![SenderName::for_test("new source")]);
        assert!(receiver.next_poll().unwrap() <= Instant::now());
        assert!(receiver.next_discovery <= Instant::now());
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
}
