use crate::spout::{SenderIdentity, SenderName, SpoutReceiver};
use std::time::{Duration, Instant};

const RECONNECT_RETRY: Duration = Duration::from_millis(500);

struct ReconnectRetry {
    identity: SenderIdentity,
    deadline: Instant,
}

pub struct SourceController {
    receiver: SpoutReceiver,
    retry: Option<ReconnectRetry>,
}

impl SourceController {
    pub fn new() -> Self {
        Self {
            receiver: SpoutReceiver::new(),
            retry: None,
        }
    }

    pub fn update_discovered(&mut self, senders: Vec<SenderName>) {
        self.receiver.update_discovered(senders);
    }

    pub fn select(&mut self, sender: SenderName) {
        self.receiver.select(sender);
        self.retry = None;
    }

    pub fn poll(&mut self) -> std::io::Result<bool> {
        self.receiver.poll()
    }
    pub fn identity(&self) -> Option<SenderIdentity> {
        self.receiver.identity()
    }

    pub fn matches_installed(&self, installed: Option<&SenderIdentity>) -> bool {
        installed.is_some_and(|identity| self.receiver.matches(identity))
    }

    pub fn can_open(&self, identity: &SenderIdentity, now: Instant) -> bool {
        self.retry
            .as_ref()
            .is_none_or(|retry| &retry.identity != identity || retry.deadline <= now)
    }

    pub fn failed(&mut self, identity: SenderIdentity, now: Instant) {
        self.retry = Some(ReconnectRetry {
            identity,
            deadline: now + RECONNECT_RETRY,
        });
    }

    pub fn installed(&mut self) {
        self.retry = None;
    }

    pub fn next_wake(&self, installed: Option<&SenderIdentity>, now: Instant) -> Option<Instant> {
        let reconnect = self
            .identity()
            .filter(|desired| Some(desired) != installed)
            .map(|desired| {
                self.retry
                    .as_ref()
                    .filter(|retry| retry.identity == desired)
                    .map_or(now, |retry| retry.deadline)
            });
        self.receiver.next_poll().into_iter().chain(reconnect).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(generation: u64) -> SenderIdentity {
        SenderIdentity {
            name: SenderName::for_test("source"),
            share_handle: 7,
            generation,
        }
    }

    #[test]
    fn retry_is_scoped_to_the_failed_resource_generation() {
        let now = Instant::now();
        let mut source = SourceController::new();
        let old = identity(1);
        source.failed(old.clone(), now);
        assert!(!source.can_open(&old, now + Duration::from_millis(499)));
        assert!(source.can_open(&old, now + Duration::from_millis(500)));
        assert!(source.can_open(&identity(2), now));
        source.installed();
        assert!(source.can_open(&old, now));
    }

    #[test]
    fn no_sources_need_no_periodic_ui_wake() {
        let source = SourceController::new();
        assert_eq!(source.next_wake(None, Instant::now()), None);
        assert!(!source.matches_installed(None));
    }
}
