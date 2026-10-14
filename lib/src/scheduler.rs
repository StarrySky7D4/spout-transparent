use std::time::Instant;

#[derive(Clone, Copy)]
pub enum Task {
    Source,
    Frame,
    Readback,
    Layout,
    AlphaSetup,
}

#[derive(Default)]
pub struct Scheduler {
    deadlines: [Option<Instant>; 5],
}

impl Scheduler {
    pub fn set(&mut self, task: Task, deadline: Option<Instant>) {
        self.deadlines[task as usize] = deadline;
    }

    pub fn take_due(&mut self, task: Task, now: Instant) -> bool {
        let slot = &mut self.deadlines[task as usize];
        if slot.is_some_and(|deadline| deadline <= now) {
            *slot = None;
            true
        } else {
            false
        }
    }

    pub fn next_wake(&self) -> Option<Instant> {
        self.deadlines.iter().flatten().copied().min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn independent_deadlines_choose_only_the_earliest_active_task() {
        let now = Instant::now();
        let mut s = Scheduler::default();
        assert_eq!(s.next_wake(), None);
        s.set(Task::Frame, Some(now + Duration::from_millis(33)));
        s.set(Task::Readback, Some(now + Duration::from_millis(2)));
        s.set(Task::Source, Some(now + Duration::from_millis(100)));
        assert_eq!(s.next_wake(), Some(now + Duration::from_millis(2)));
        assert!(!s.take_due(Task::Frame, now));
        assert!(s.take_due(Task::Readback, now + Duration::from_millis(2)));
        assert_eq!(s.next_wake(), Some(now + Duration::from_millis(33)));
        s.set(Task::Frame, None);
        assert_eq!(s.next_wake(), Some(now + Duration::from_millis(100)));
    }

    #[test]
    fn consumed_overdue_tasks_do_not_keep_the_loop_busy() {
        let now = Instant::now();
        let mut s = Scheduler::default();
        s.set(Task::Layout, Some(now));
        assert!(s.take_due(Task::Layout, now));
        assert!(!s.take_due(Task::Layout, now));
        assert_eq!(s.next_wake(), None);
    }
}
