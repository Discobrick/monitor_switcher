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
///
/// `tick` must be called periodically (not just `evaluate`) for a presence
/// change that arrived mid-cooldown to ever be delivered: `evaluate` alone
/// only reacts to a *new* reading, so a flip that lands during cooldown and
/// is never followed by another `evaluate` call would otherwise sit applied
/// only in `last_state` forever. The watcher thread (Task 8) is expected to
/// call `tick` on every poll, the same as `evaluate`.
#[derive(Debug)]
pub struct WatcherState {
    cooldown: Duration,
    /// `None` until the first evaluation, which only establishes a baseline.
    last_state: Option<bool>,
    /// The presence value the monitors currently reflect (last `Connect`,
    /// `Disconnect`, or `Resync` applied it). Compared against `last_state`
    /// at cooldown expiry to decide whether a resync is actually needed —
    /// this is what a bare "something changed" flag can't tell apart from
    /// "changed and changed back".
    applied: Option<bool>,
    cooldown_until: Option<Instant>,
}

impl WatcherState {
    pub fn new(cooldown: Duration) -> Self {
        Self { cooldown, last_state: None, applied: None, cooldown_until: None }
    }

    /// Sets the cooldown used by the *next* switch.
    ///
    /// `cooldown_until` is an absolute instant computed once, at the moment
    /// a switch happens (`now + cooldown`). Calling this mid-cooldown does
    /// not retroactively shorten or lengthen the cooldown already in
    /// flight — it only takes effect the next time a switch (or resync)
    /// arms a new one.
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
            self.applied = Some(present);
            return None;
        };

        if self.in_cooldown(now) {
            if present != previous {
                self.last_state = Some(present);
            }
            return None;
        }

        if present == previous {
            return None;
        }

        self.last_state = Some(present);
        self.applied = Some(present);
        self.cooldown_until = Some(self.arm(now));

        Some(if present { Action::Connect } else { Action::Disconnect })
    }

    /// Called periodically by the watcher thread to release a cooldown.
    pub fn tick(&mut self, now: Instant) -> Option<Action> {
        if self.in_cooldown(now) {
            return None;
        }

        let expired = self.cooldown_until.take().is_some();
        if expired && self.last_state != self.applied {
            self.applied = self.last_state;
            self.cooldown_until = Some(self.arm(now));
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

    /// `now + self.cooldown`, saturating instead of panicking if a
    /// pathological (hand-edited) config value would overflow `Instant`.
    fn arm(&self, now: Instant) -> Instant {
        now.checked_add(self.cooldown).unwrap_or(now)
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

    // --- Fix round 1 findings ---

    #[test]
    fn a_flip_that_returns_to_the_already_applied_state_during_cooldown_is_not_resynced() {
        let (mut s, t0) = state();
        s.evaluate(true, t0); // baseline
        s.evaluate(false, t0 + Duration::from_secs(1)); // -> Disconnect, applied = false
        s.evaluate(true, t0 + Duration::from_secs(5)); // suppressed
        s.evaluate(false, t0 + Duration::from_secs(10)); // flipped BACK to the applied state

        // The monitors already show the right input: no phantom resync, no
        // fresh cooldown burned.
        assert_eq!(s.tick(t0 + Duration::from_secs(61)), None);
        assert_eq!(s.cooldown_remaining(t0 + Duration::from_secs(61)), None);
    }

    #[test]
    fn last_state_reflects_true_presence_while_suppressed_during_cooldown() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));
        s.evaluate(true, t0 + Duration::from_secs(5)); // suppressed

        assert_eq!(s.last_state(), Some(true));
    }

    #[test]
    fn evaluate_at_expiry_returns_none_when_presence_already_matches_applied() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1)); // applied = false
        s.evaluate(true, t0 + Duration::from_secs(5)); // suppressed
        s.evaluate(false, t0 + Duration::from_secs(10)); // back to applied value

        // A fresh reading exactly at expiry, still matching what's applied:
        // must not be reported as a switch.
        assert_eq!(s.evaluate(false, t0 + Duration::from_secs(61)), None);
    }

    #[test]
    fn no_phantom_resync_after_a_switch_resolves_a_pending_flip() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1)); // switch #1, cooldown until t0+61
        s.evaluate(true, t0 + Duration::from_secs(5)); // suppressed, pending

        // Resync consumes the pending flip and re-arms the cooldown.
        assert_eq!(s.tick(t0 + Duration::from_secs(61)), Some(Action::Resync));

        // A later, genuinely new switch after that cooldown clears...
        assert_eq!(s.evaluate(false, t0 + Duration::from_secs(130)), Some(Action::Disconnect));

        // ...must not leave anything behind for the NEXT expiry to resync.
        assert_eq!(s.tick(t0 + Duration::from_secs(191)), None);
    }

    #[test]
    fn set_cooldown_does_not_affect_an_in_flight_cooldown() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1)); // cooldown_until = t0 + 61s

        s.set_cooldown(Duration::from_secs(5)); // shorten mid-cooldown
        assert_eq!(
            s.cooldown_remaining(t0 + Duration::from_secs(11)).unwrap().as_secs(),
            50,
            "the in-flight cooldown must still use the old duration"
        );

        // It applies starting with the next switch.
        assert_eq!(s.evaluate(true, t0 + Duration::from_secs(70)), Some(Action::Connect));
        assert_eq!(
            s.cooldown_remaining(t0 + Duration::from_secs(71)).unwrap().as_secs(),
            4,
            "the new cooldown value is used for the next switch"
        );
    }

    #[test]
    fn set_cooldown_lengthening_also_only_applies_to_the_next_switch() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1)); // cooldown_until = t0 + 61s

        s.set_cooldown(Duration::from_secs(120)); // lengthen mid-cooldown

        // Still expires per the original 60s cooldown, not the new 120s one.
        assert_eq!(s.cooldown_remaining(t0 + Duration::from_secs(61)), None);

        assert_eq!(s.evaluate(true, t0 + Duration::from_secs(61)), Some(Action::Connect));
        assert_eq!(
            s.cooldown_remaining(t0 + Duration::from_secs(62)).unwrap().as_secs(),
            119,
            "the lengthened cooldown is used for the next switch"
        );
    }

    #[test]
    fn an_absurd_cooldown_saturates_instead_of_panicking_on_overflow() {
        let mut s = WatcherState::new(Duration::from_secs(u64::MAX));
        let t0 = Instant::now();
        s.evaluate(true, t0);

        // Must not panic ("overflow when adding duration to instant").
        let action = s.evaluate(false, t0 + Duration::from_secs(1));
        assert_eq!(action, Some(Action::Disconnect));
    }
}
