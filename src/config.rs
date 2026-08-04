use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRate {
    Unlimited,
    Fps30,
    Fps60,
    Fps120,
}

impl FrameRate {
    pub fn cycle(self) -> Self {
        match self {
            Self::Unlimited => Self::Fps120,
            Self::Fps120 => Self::Fps60,
            Self::Fps60 => Self::Fps30,
            Self::Fps30 => Self::Unlimited,
        }
    }

    pub fn interval(self) -> Option<Duration> {
        match self {
            Self::Unlimited => None,
            Self::Fps30 => Some(Duration::from_nanos(33_333_333)),
            Self::Fps60 => Some(Duration::from_nanos(16_666_667)),
            Self::Fps120 => Some(Duration::from_nanos(8_333_333)),
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Unlimited => "Unlimited",
            Self::Fps30 => "30fps",
            Self::Fps60 => "60fps",
            Self::Fps120 => "120fps",
        }
    }
}

pub struct FramePacer {
    rate: FrameRate,
    last_presented_at: Instant,
    force_next: bool,
}

impl FramePacer {
    const BUSY_RETRY: Duration = Duration::from_millis(1);
    const DISCONNECTED_RETRY: Duration = Duration::from_millis(16);

    pub fn new(now: Instant) -> Self {
        Self {
            rate: FrameRate::Unlimited,
            last_presented_at: now,
            force_next: true,
        }
    }

    pub fn cycle(&mut self) -> FrameRate {
        self.rate = self.rate.cycle();
        self.force_next = true;
        self.rate
    }

    pub fn request_frame(&mut self) {
        self.force_next = true;
    }

    pub fn is_due(&self, now: Instant) -> bool {
        self.force_next
            || self
                .rate
                .interval()
                .is_none_or(|interval| now.duration_since(self.last_presented_at) >= interval)
    }

    pub fn presented(&mut self, now: Instant) {
        self.last_presented_at = now;
        self.force_next = false;
    }

    pub fn next_wake(
        &self,
        now: Instant,
        sender_connected: bool,
        presented_frame: bool,
    ) -> Option<Instant> {
        if !sender_connected {
            return Some(now + Self::DISCONNECTED_RETRY);
        }
        if !presented_frame && self.is_due(now) {
            return Some(now + Self::BUSY_RETRY);
        }

        self.rate.interval().map(|interval| {
            let deadline = self.last_presented_at + interval;
            if deadline > now {
                deadline
            } else {
                now + Self::BUSY_RETRY
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_rate_requests_an_immediate_frame() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        pacer.presented(start);
        assert_eq!(pacer.cycle(), FrameRate::Fps120);
        assert!(pacer.is_due(start));
    }

    #[test]
    fn capped_rate_waits_after_a_presented_frame() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        pacer.cycle();
        pacer.presented(start);

        assert!(!pacer.is_due(start + Duration::from_millis(1)));
        assert!(pacer.is_due(start + Duration::from_millis(9)));
    }

    #[test]
    fn capped_rate_returns_its_next_deadline() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        pacer.cycle();
        pacer.presented(start);

        assert_eq!(
            pacer.next_wake(start, true, true),
            Some(start + FrameRate::Fps120.interval().unwrap())
        );
    }
}
