//! When to run the next bounded sync round. Holds backoff, retry, interval and
//! terminal states; the caller owns the timer, the round and its reporting.
use std::time::Duration;

use tokio::time::Instant;

/// Bounded sync rounds per wake.
pub const ROUND_BUDGET: usize = 8;

/// Delay after another process held the sync lock.
const CONTENTION_RESCHEDULE: Duration = Duration::from_secs(1);
/// Delay before continuing a wake that stopped at its round budget.
const INCOMPLETE_RESCHEDULE: Duration = Duration::from_millis(100);
const MAX_BACKOFF_SECONDS: u64 = 300;

pub struct SyncScheduler {
    interval: Duration,
    next_sync: Instant,
    backoff_seconds: u64,
    /// While set, an ordinary wake must not run a round before this time.
    retry_not_before: Option<Instant>,
    catching_up: bool,
    awaiting_setup: bool,
    access_refused: bool,
    vault_deleted: bool,
}

impl SyncScheduler {
    /// The first round is due at `now`.
    pub fn new(interval: Duration, now: Instant) -> Self {
        Self {
            interval,
            next_sync: now,
            backoff_seconds: 1,
            retry_not_before: None,
            catching_up: false,
            awaiting_setup: false,
            access_refused: false,
            vault_deleted: false,
        }
    }

    pub fn next_sync(&self) -> Instant {
        self.next_sync
    }

    /// The last round stopped at its budget with more work ready.
    pub fn catching_up(&self) -> bool {
        self.catching_up
    }

    /// Brings the next round forward unless a retry delay forbids it. `retry_now`
    /// bypasses that delay.
    pub fn wake(&mut self, retry_now: bool, now: Instant) {
        if wake_can_sync(retry_now, self.retry_not_before, now) {
            self.retry_not_before = None;
            self.next_sync = now;
        }
    }

    pub fn round_started(&mut self) {
        self.retry_not_before = None;
    }

    /// Returns true when the round left nothing to do.
    pub fn completed(&mut self, more_work: bool, now: Instant) -> bool {
        self.catching_up = more_work;
        self.backoff_seconds = 1;
        self.clear_terminal_states();
        self.next_sync = now
            + if more_work {
                INCOMPLETE_RESCHEDULE
            } else {
                self.interval
            };
        !more_work
    }

    /// Returns true the first time this state is reached.
    pub fn not_set_up(&mut self, now: Instant) -> bool {
        self.catching_up = false;
        self.access_refused = false;
        self.vault_deleted = false;
        self.backoff_seconds = 1;
        self.next_sync = now + self.interval;
        !std::mem::replace(&mut self.awaiting_setup, true)
    }

    pub fn deferred(&mut self, now: Instant) {
        self.catching_up = false;
        self.next_sync = now + CONTENTION_RESCHEDULE;
    }

    /// Rounds check the recorded deletion locally and send no request until the
    /// user resets sync or a foreground sync succeeds. Returns true the first
    /// time this state is reached.
    pub fn vault_deleted(&mut self, now: Instant) -> bool {
        self.catching_up = false;
        self.backoff_seconds = 1;
        self.next_sync = now + self.interval;
        !std::mem::replace(&mut self.vault_deleted, true)
    }

    /// Refusals persist until the user acts elsewhere, so backing off only
    /// repeats the warning. Returns true the first time this state is reached.
    pub fn access_refused(&mut self, now: Instant) -> bool {
        self.catching_up = false;
        self.backoff_seconds = 1;
        self.next_sync = now + self.interval;
        self.retry_not_before = Some(self.next_sync);
        !std::mem::replace(&mut self.access_refused, true)
    }

    /// Returns the delay before the retry, in seconds.
    pub fn failed(&mut self, now: Instant) -> u64 {
        self.catching_up = false;
        let retry_seconds = self.backoff_seconds;
        self.backoff_seconds = (self.backoff_seconds * 2).min(MAX_BACKOFF_SECONDS);
        self.next_sync = now + Duration::from_secs(retry_seconds);
        self.retry_not_before = Some(self.next_sync);
        retry_seconds
    }

    fn clear_terminal_states(&mut self) {
        self.awaiting_setup = false;
        self.access_refused = false;
        self.vault_deleted = false;
    }
}

fn wake_can_sync(retry_now: bool, deadline: Option<Instant>, now: Instant) -> bool {
    retry_now || deadline.is_none_or(|deadline| now >= deadline)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(30);

    fn scheduler() -> (SyncScheduler, Instant) {
        let now = Instant::now();
        (SyncScheduler::new(INTERVAL, now), now)
    }

    #[test]
    fn only_retry_wakes_bypass_backoff() {
        let now = Instant::now();
        let deadline = Some(now + Duration::from_secs(300));
        assert!(!wake_can_sync(false, deadline, now));
        assert!(wake_can_sync(true, deadline, now));
        assert!(wake_can_sync(
            false,
            deadline,
            now + Duration::from_secs(300)
        ));
        assert!(wake_can_sync(false, None, now));
    }

    #[test]
    fn first_round_is_due_immediately() {
        let (scheduler, now) = scheduler();
        assert_eq!(scheduler.next_sync(), now);
        assert!(!scheduler.catching_up());
    }

    #[test]
    fn caught_up_rounds_wait_for_the_interval() {
        let (mut scheduler, now) = scheduler();
        assert!(scheduler.completed(false, now));
        assert_eq!(scheduler.next_sync(), now + INTERVAL);
        assert!(!scheduler.catching_up());
    }

    #[test]
    fn rounds_with_more_work_continue_quickly() {
        let (mut scheduler, now) = scheduler();
        assert!(!scheduler.completed(true, now));
        assert_eq!(scheduler.next_sync(), now + INCOMPLETE_RESCHEDULE);
        assert!(scheduler.catching_up());
        scheduler.deferred(now);
        assert!(!scheduler.catching_up());
    }

    #[test]
    fn failures_back_off_to_a_cap_and_success_resets() {
        let (mut scheduler, now) = scheduler();
        let delays = (0..11).map(|_| scheduler.failed(now)).collect::<Vec<_>>();
        assert_eq!(delays, [1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
        assert_eq!(scheduler.next_sync(), now + Duration::from_secs(300));
        scheduler.completed(false, now);
        assert_eq!(scheduler.failed(now), 1);
    }

    #[test]
    fn terminal_states_reset_backoff() {
        for reach in [
            SyncScheduler::not_set_up,
            SyncScheduler::vault_deleted,
            SyncScheduler::access_refused,
        ] {
            let (mut scheduler, now) = scheduler();
            scheduler.failed(now);
            reach(&mut scheduler, now);
            assert_eq!(scheduler.next_sync(), now + INTERVAL);
            assert_eq!(scheduler.failed(now), 1);
        }
    }

    #[test]
    fn contention_keeps_backoff_and_retries_shortly() {
        let (mut scheduler, now) = scheduler();
        scheduler.failed(now);
        scheduler.deferred(now);
        assert_eq!(scheduler.next_sync(), now + CONTENTION_RESCHEDULE);
        assert_eq!(scheduler.failed(now), 2);
    }

    #[test]
    fn terminal_states_report_once_until_they_change() {
        let (mut scheduler, now) = scheduler();
        assert!(scheduler.not_set_up(now));
        assert!(!scheduler.not_set_up(now));
        assert!(scheduler.access_refused(now));
        assert!(!scheduler.access_refused(now));
        assert!(scheduler.vault_deleted(now));
        assert!(!scheduler.vault_deleted(now));
        // Not-set-up clears the others, but they do not clear it.
        assert!(!scheduler.not_set_up(now));
        assert!(scheduler.access_refused(now));
        scheduler.completed(false, now);
        assert!(scheduler.not_set_up(now));
        scheduler.completed(false, now);
        assert!(scheduler.vault_deleted(now));
    }

    #[test]
    fn deferred_rounds_keep_terminal_reports_quiet() {
        let (mut scheduler, now) = scheduler();
        assert!(scheduler.vault_deleted(now));
        scheduler.deferred(now);
        assert!(!scheduler.vault_deleted(now));
    }

    #[test]
    fn ordinary_wakes_wait_out_a_failure_but_retry_wakes_do_not() {
        let (mut scheduler, now) = scheduler();
        for _ in 0..4 {
            scheduler.failed(now);
        }
        let due = scheduler.next_sync();
        scheduler.wake(false, now);
        assert_eq!(scheduler.next_sync(), due);
        scheduler.wake(true, now);
        assert_eq!(scheduler.next_sync(), now);
    }

    #[test]
    fn ordinary_wakes_run_once_the_retry_delay_passes() {
        let (mut scheduler, now) = scheduler();
        scheduler.failed(now);
        let due = scheduler.next_sync();
        scheduler.wake(false, due);
        assert_eq!(scheduler.next_sync(), due);
        scheduler.failed(now);
        scheduler.round_started();
        scheduler.wake(false, now);
        assert_eq!(scheduler.next_sync(), now);
    }

    #[test]
    fn refusals_hold_ordinary_wakes_for_the_interval() {
        let (mut scheduler, now) = scheduler();
        scheduler.access_refused(now);
        scheduler.wake(false, now);
        assert_eq!(scheduler.next_sync(), now + INTERVAL);
        scheduler.wake(true, now);
        assert_eq!(scheduler.next_sync(), now);
    }

    #[test]
    fn terminal_states_without_a_retry_delay_allow_wakes() {
        let (mut scheduler, now) = scheduler();
        scheduler.not_set_up(now);
        scheduler.wake(false, now);
        assert_eq!(scheduler.next_sync(), now);
        scheduler.vault_deleted(now);
        scheduler.wake(false, now);
        assert_eq!(scheduler.next_sync(), now);
    }
}
