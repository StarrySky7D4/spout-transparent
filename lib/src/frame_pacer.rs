//! Presentation budgets start at frame submission. Unchanged senders back off;
//! contended access locks use a separate short retry.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    Presented,
    SenderUnchanged,
    Busy,
}

pub struct FramePacer {
    rate: FrameRate,
    last_frame_started_at: Instant,
    force_next: bool,
    idle_retry: Duration,
}

impl FramePacer {
    const BUSY_RETRY: Duration = Duration::from_millis(1);
    const MAX_IDLE_RETRY: Duration = Duration::from_millis(8);

    pub fn new(now: Instant) -> Self {
        Self {
            rate: FrameRate::Unlimited,
            last_frame_started_at: now,
            force_next: true,
            idle_retry: Self::BUSY_RETRY,
        }
    }

    pub fn cycle(&mut self) -> FrameRate {
        self.set_rate(self.rate.cycle());
        self.rate
    }

    pub fn set_rate(&mut self, rate: FrameRate) {
        if self.rate != rate {
            self.rate = rate;
            self.request_frame();
        }
    }

    pub fn rate(&self) -> FrameRate {
        self.rate
    }

    pub fn request_frame(&mut self) {
        self.force_next = true;
        self.idle_retry = Self::BUSY_RETRY;
    }

    pub fn is_due(&self, now: Instant) -> bool {
        self.force_next
            || self
                .rate
                .interval()
                .is_none_or(|interval| now.duration_since(self.last_frame_started_at) >= interval)
    }

    pub fn presented(&mut self, frame_started_at: Instant) {
        self.last_frame_started_at = frame_started_at;
        self.force_next = false;
        self.idle_retry = Self::BUSY_RETRY;
    }

    pub fn next_wake(
        &mut self,
        now: Instant,
        active: bool,
        outcome: FrameOutcome,
    ) -> Option<Instant> {
        if !active {
            self.idle_retry = Self::BUSY_RETRY;
            return None;
        }
        if outcome != FrameOutcome::Presented && self.is_due(now) {
            // Back off only after confirming that the sender has no new frame.
            // Contended texture locks still retry promptly.
            let retry = if outcome == FrameOutcome::SenderUnchanged {
                let retry = self.idle_retry;
                self.idle_retry = (retry * 2).min(Self::MAX_IDLE_RETRY);
                retry
            } else {
                self.idle_retry = Self::BUSY_RETRY;
                Self::BUSY_RETRY
            };
            return Some(now + retry);
        }

        Some(self.rate.interval().map_or(now, |interval| {
            let deadline = self.last_frame_started_at + interval;
            if deadline > now {
                deadline
            } else {
                now + Self::BUSY_RETRY
            }
        }))
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
            pacer.next_wake(start, true, FrameOutcome::Presented),
            Some(start + FrameRate::Fps120.interval().unwrap())
        );
    }

    #[test]
    fn explicit_rate_selection_updates_the_pacer() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        pacer.presented(start);
        pacer.set_rate(FrameRate::Fps30);

        assert_eq!(pacer.rate(), FrameRate::Fps30);
        assert!(pacer.is_due(start));
    }

    #[test]
    fn unchanged_sender_backs_off_with_a_bounded_delay() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        pacer.presented(start);
        let mut now = start;
        for millis in [1, 2, 4, 8, 8, 8] {
            let deadline = pacer
                .next_wake(now, true, FrameOutcome::SenderUnchanged)
                .unwrap();
            assert_eq!(deadline - now, Duration::from_millis(millis));
            now = deadline;
        }
        assert!(pacer.is_due(now));
    }

    #[test]
    fn busy_texture_does_not_use_idle_sender_backoff() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        for _ in 0..5 {
            pacer.next_wake(start, true, FrameOutcome::SenderUnchanged);
        }
        assert_eq!(
            pacer.next_wake(start, true, FrameOutcome::Busy),
            Some(start + Duration::from_millis(1))
        );
    }

    #[test]
    fn presentation_and_explicit_redraw_reset_idle_backoff() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        for reset_by_present in [false, true] {
            for _ in 0..5 {
                pacer.next_wake(start, true, FrameOutcome::SenderUnchanged);
            }
            if reset_by_present {
                pacer.presented(start);
            } else {
                pacer.request_frame();
            }
            assert_eq!(
                pacer.next_wake(start, true, FrameOutcome::SenderUnchanged),
                Some(start + Duration::from_millis(1))
            );
        }
    }

    #[test]
    fn capped_rate_backs_off_when_sender_stops_after_the_deadline() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        pacer.set_rate(FrameRate::Fps30);
        pacer.presented(start);
        let mut now = start + FrameRate::Fps30.interval().unwrap();
        for millis in [1, 2, 4, 8] {
            assert!(pacer.is_due(now));
            let next = pacer
                .next_wake(now, true, FrameOutcome::SenderUnchanged)
                .unwrap();
            assert_eq!(next - now, Duration::from_millis(millis));
            now = next;
        }
    }

    #[test]
    fn inactive_renderer_has_no_frame_deadline() {
        let now = Instant::now();
        let mut pacer = FramePacer::new(now);
        assert_eq!(pacer.next_wake(now, false, FrameOutcome::Busy), None);
        pacer.presented(now);
        assert_eq!(
            pacer.next_wake(now, true, FrameOutcome::Presented),
            Some(now)
        );
    }

    #[test]
    fn present_cost_is_part_of_the_capped_frame_budget() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        pacer.set_rate(FrameRate::Fps60);
        let completed = start + Duration::from_millis(10);
        pacer.presented(start);
        assert_eq!(
            pacer.next_wake(completed, true, FrameOutcome::Presented),
            Some(start + FrameRate::Fps60.interval().unwrap())
        );
    }
}
