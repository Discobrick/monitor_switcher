use std::time::{Duration, Instant};

/// What the caller should do to the monitors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Apply every rule's `on_connect` value.
    Connect,
    /// Apply every rule's `on_disconnect` value.
    Disconnect,
    /// Re-apply whichever set matches the current presence.
    ///
    /// Emitted when the device state changed during a cooldown; without this
    /// the monitors would be left showing the wrong input indefinitely.
    Resync,
}

/// Presence tracking with debounce-independent cooldown handling.
///
/// Deliberately free of I/O: it takes a `bool` and an `Instant`, so every
/// transition, suppression and resync is testable on any platform.
#[derive(Debug)]
pub struct WatcherState {
    cooldown: Duration,
    /// `None` until the first evaluation, which only establishes a baseline.
    last_state: Option<bool>,
    cooldown_until: Option<Instant>,
    /// Set when a change arrived during cooldown and still needs applying.
    dirty: bool,
}

impl WatcherState {
    pub fn new(cooldown: Duration) -> Self {
        Self { cooldown, last_state: None, cooldown_until: None, dirty: false }
    }

    pub fn set_cooldown(&mut self, cooldown: Duration) {
        self.cooldown = cooldown;
    }

    pub fn last_state(&self) -> Option<bool> {
        self.last_state
    }

    /// Feeds an observed presence reading in.
    ///
    /// The very first call only records the baseline: the app must not switch
    /// monitors merely because it started up.
    pub fn evaluate(&mut self, present: bool, now: Instant) -> Option<Action> {
        let Some(previous) = self.last_state else {
            self.last_state = Some(present);
            return None;
        };

        if self.in_cooldown(now) {
            if present != previous {
                self.dirty = true;
                self.last_state = Some(present);
            }
            return None;
        }

        if present == previous {
            return None;
        }

        self.last_state = Some(present);
        self.cooldown_until = Some(now + self.cooldown);
        self.dirty = false;

        Some(if present { Action::Connect } else { Action::Disconnect })
    }

    /// Called periodically by the watcher thread to release a cooldown.
    pub fn tick(&mut self, now: Instant) -> Option<Action> {
        if self.in_cooldown(now) {
            return None;
        }

        let expired = self.cooldown_until.take().is_some();
        if expired && self.dirty {
            self.dirty = false;
            self.cooldown_until = Some(now + self.cooldown);
            return Some(Action::Resync);
        }
        None
    }

    pub fn cooldown_remaining(&self, now: Instant) -> Option<Duration> {
        self.cooldown_until.filter(|until| *until > now).map(|until| until - now)
    }

    fn in_cooldown(&self, now: Instant) -> bool {
        self.cooldown_until.is_some_and(|until| now < until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn state() -> (WatcherState, Instant) {
        (WatcherState::new(Duration::from_secs(60)), Instant::now())
    }

    #[test]
    fn first_evaluation_establishes_state_without_switching() {
        let (mut s, t0) = state();
        assert_eq!(s.evaluate(true, t0), None, "initial sync must not switch");
    }

    #[test]
    fn a_transition_produces_the_matching_action() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);

        assert_eq!(s.evaluate(false, t0 + Duration::from_secs(1)), Some(Action::Disconnect));
    }

    #[test]
    fn no_action_when_state_is_unchanged() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        assert_eq!(s.evaluate(true, t0 + Duration::from_secs(1)), None);
    }

    #[test]
    fn a_switch_starts_a_cooldown_that_suppresses_further_switches() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        assert_eq!(s.evaluate(false, t0 + Duration::from_secs(1)), Some(Action::Disconnect));

        // Flip back inside the cooldown: suppressed.
        assert_eq!(s.evaluate(true, t0 + Duration::from_secs(5)), None);
        assert_eq!(s.evaluate(false, t0 + Duration::from_secs(10)), None);
    }

    #[test]
    fn a_flip_during_cooldown_is_honoured_at_expiry() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1)); // switch, cooldown starts
        s.evaluate(true, t0 + Duration::from_secs(5)); // suppressed, marks dirty

        // At expiry the real state is re-applied rather than left wrong.
        assert_eq!(s.tick(t0 + Duration::from_secs(62)), Some(Action::Resync));
    }

    #[test]
    fn no_resync_when_nothing_happened_during_cooldown() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));

        assert_eq!(s.tick(t0 + Duration::from_secs(62)), None);
    }

    #[test]
    fn tick_before_expiry_does_nothing() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));
        s.evaluate(true, t0 + Duration::from_secs(5));

        assert_eq!(s.tick(t0 + Duration::from_secs(30)), None);
    }

    #[test]
    fn cooldown_remaining_counts_down_then_clears() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));

        let left = s.cooldown_remaining(t0 + Duration::from_secs(11)).unwrap();
        assert_eq!(left.as_secs(), 50);
        assert!(s.cooldown_remaining(t0 + Duration::from_secs(120)).is_none());
    }

    #[test]
    fn after_cooldown_expires_switching_resumes_normally() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));

        assert_eq!(s.evaluate(true, t0 + Duration::from_secs(70)), Some(Action::Connect));
    }

    // --- Boundary tests: pin behaviour exactly at cooldown expiry. ---
    // cooldown_until is set to `now + cooldown` at the switch; `in_cooldown`
    // uses a strict `now < until`, so the instant equal to `cooldown_until`
    // must already count as expired, not suppressed.

    #[test]
    fn evaluate_at_the_exact_cooldown_expiry_instant_is_not_suppressed() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1)); // cooldown_until = t0 + 61s

        // now == cooldown_until exactly: must be treated as expired.
        assert_eq!(
            s.evaluate(true, t0 + Duration::from_secs(61)),
            Some(Action::Connect),
            "the exact expiry instant must not be treated as still-in-cooldown"
        );
    }

    #[test]
    fn tick_at_the_exact_cooldown_expiry_instant_resolves_a_pending_dirty_flip() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1)); // cooldown_until = t0 + 61s
        s.evaluate(true, t0 + Duration::from_secs(5)); // suppressed, marks dirty

        // now == cooldown_until exactly: expiry, not "one tick before".
        assert_eq!(s.tick(t0 + Duration::from_secs(61)), Some(Action::Resync));
    }

    #[test]
    fn cooldown_remaining_is_none_at_the_exact_expiry_instant() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1)); // cooldown_until = t0 + 61s

        assert!(s.cooldown_remaining(t0 + Duration::from_secs(61)).is_none());
    }

    #[test]
    fn a_resync_at_expiry_starts_a_fresh_cooldown_that_still_suppresses() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));
        s.evaluate(true, t0 + Duration::from_secs(5)); // dirty flip during cooldown

        assert_eq!(s.tick(t0 + Duration::from_secs(62)), Some(Action::Resync));

        // The resync itself re-armed the cooldown; a flip right after is suppressed.
        assert_eq!(s.evaluate(false, t0 + Duration::from_secs(63)), None);
    }
}
