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

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use crate::app::{Command, Event, LogEntry, Severity};
use crate::config::Config;
use crate::hardware;

/// Current time of day, UTC, formatted `HH:MM:SS`. No timezone conversion —
/// this is a display timestamp, not a wall-clock claim.
fn now_string() -> String {
    // ponytail: no chrono dependency for one timestamp. SystemTime -> HH:MM:SS
    // via seconds-of-day arithmetic; the date is never displayed.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let sod = secs % 86_400;
    format!("{:02}:{:02}:{:02}", sod / 3600, (sod % 3600) / 60, sod % 60)
}

pub fn log(tx: &Sender<Event>, severity: Severity, message: impl Into<String>) {
    let _ = tx.send(Event::Log(LogEntry {
        at: now_string(),
        severity,
        message: message.into(),
    }));
}

/// Applies every configured rule for the given presence state.
fn apply_all(cfg: &Config, dir: &PathBuf, present: bool, tx: &Sender<Event>) {
    let tool = crate::app::tool_path(cfg, dir);

    for rule in &cfg.monitors {
        if rule.serial.is_empty() {
            log(
                tx,
                Severity::Warning,
                format!("Skipping \"{}\": no serial, re-detect it in Monitors", rule.label),
            );
            continue;
        }

        let value = if present { rule.on_connect } else { rule.on_disconnect };
        match hardware::apply_input(&tool, &rule.serial, value) {
            Ok(()) => log(tx, Severity::Success, format!("{} -> input {}", rule.label, value)),
            Err(e) => log(tx, Severity::Error, format!("{} failed: {e}", rule.label)),
        }
    }
}

fn any_watched_present(cfg: &Config) -> bool {
    match hardware::list_devices() {
        Ok(devices) => devices.iter().any(|d| cfg.watches(&d.id)),
        Err(_) => false,
    }
}

/// Runs the watcher loop until a `Command::Shutdown` arrives, or `commands`
/// disconnects (the UI side was dropped, e.g. during shutdown) — both are a
/// normal exit, not an error.
///
/// `wake` is signalled by the platform layer whenever a device change is
/// observed; on Windows that is the hidden window's WM_DEVICECHANGE handler,
/// on other targets it is the debug panel.
pub fn run(
    cfg: Arc<Mutex<Config>>,
    dir: PathBuf,
    tx: Sender<Event>,
    commands: Receiver<Command>,
    wake: Receiver<()>,
) {
    let cooldown = {
        let c = cfg.lock().expect("config poisoned");
        Duration::from_secs(c.cooldown_secs)
    };
    let mut state = WatcherState::new(cooldown);
    let mut enabled = cfg.lock().expect("config poisoned").monitoring_enabled;

    // Establish the baseline without switching anything.
    {
        let c = cfg.lock().expect("config poisoned");
        let present = any_watched_present(&c);
        state.evaluate(present, Instant::now());
        let _ = tx.send(Event::PresenceChanged(present));
    }

    let mut last_cooldown_report: Option<u64> = None;

    loop {
        match commands.try_recv() {
            Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => return,
            Ok(Command::SetMonitoring(on)) => enabled = on,
            Ok(Command::ConfigChanged) => {
                let c = cfg.lock().expect("config poisoned");
                state.set_cooldown(Duration::from_secs(c.cooldown_secs));
                enabled = c.monitoring_enabled;
            }
            Ok(Command::Poke) | Err(TryRecvError::Empty) => {}
        }

        // Coalesce a burst of device-change notifications: a KVM toggle fires
        // several. Drain everything that arrived, then let it settle.
        let woken = wake.try_recv().is_ok();
        if woken {
            std::thread::sleep(Duration::from_millis(500));
            while wake.try_recv().is_ok() {}
        }

        let now = Instant::now();

        if enabled && woken {
            let c = cfg.lock().expect("config poisoned");
            let present = any_watched_present(&c);

            if state.last_state() != Some(present) {
                let _ = tx.send(Event::PresenceChanged(present));
            }

            if let Some(action) = state.evaluate(present, now) {
                let label = match action {
                    Action::Connect => "Watched device connected",
                    Action::Disconnect => "Watched device disconnected",
                    Action::Resync => "Resyncing after cooldown",
                };
                log(&tx, Severity::Info, label);
                apply_all(&c, &dir, present, &tx);
            }
        }

        if enabled && let Some(action) = state.tick(now) {
            debug_assert_eq!(action, Action::Resync);
            let c = cfg.lock().expect("config poisoned");
            let present = any_watched_present(&c);
            log(&tx, Severity::Info, "Cooldown expired, resyncing monitors");
            apply_all(&c, &dir, present, &tx);
        }

        let remaining = state.cooldown_remaining(now).map(|d| d.as_secs());
        if remaining != last_cooldown_report {
            let _ = tx.send(Event::Cooldown(remaining));
            last_cooldown_report = remaining;
        }

        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Starts the platform's device-change notifier, returning the wake receiver.
///
/// On Windows this spawns the hidden-window message pump that has always
/// driven this application. On other targets there is no such thing, so the
/// sender is handed back too (Task 11's debug panel uses it to simulate a
/// device-change notification).
#[cfg(windows)]
pub fn spawn_wake_source() -> (Receiver<()>, Option<Sender<()>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || win::pump(tx));
    (rx, None)
}

#[cfg(not(windows))]
pub fn spawn_wake_source() -> (Receiver<()>, Option<Sender<()>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (rx, Some(tx))
}

/// Spawns the watcher thread and its wake source, returning the command
/// channel the caller uses to talk to it.
///
/// On non-Windows targets the debug-panel wake sender has no owner yet (it
/// arrives with Task 11's debug panel), so it is dropped here; until then the
/// watcher simply never wakes on its own outside tests, which drive `run`
/// directly.
pub fn spawn(cfg: Arc<Mutex<Config>>, dir: PathBuf, tx: Sender<Event>) -> Sender<Command> {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (wake_rx, _debug_wake) = spawn_wake_source();
    std::thread::spawn(move || run(cfg, dir, tx, cmd_rx, wake_rx));
    cmd_tx
}

#[cfg(windows)]
mod win {
    use std::sync::mpsc::Sender;
    use std::sync::OnceLock;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::HBRUSH;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
        RegisterClassW, TranslateMessage, MSG, WINDOW_EX_STYLE, WM_DESTROY, WM_DEVICECHANGE,
        WNDCLASSW, WS_OVERLAPPED,
    };

    static WAKE: OnceLock<Sender<()>> = OnceLock::new();

    const DBT_DEVNODES_CHANGED: usize = 0x0007;
    const DBT_DEVICEARRIVAL: usize = 0x8000;
    const DBT_DEVICEREMOVECOMPLETE: usize = 0x8004;

    /// Creates a hidden message-only-ish window and pumps messages forever.
    ///
    /// A top-level window receives DBT_DEVNODES_CHANGED broadcasts without
    /// RegisterDeviceNotification, which is why no registration happens here.
    pub fn pump(tx: Sender<()>) {
        let _ = WAKE.set(tx);

        // SAFETY: GetModuleHandleW(None) returns this process's base address and
        // mutates nothing.
        let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
            return;
        };

        let class_name = w!("MONITOR_SWITCHER_WATCHER");
        let wnd_class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: class_name,
            lpfnWndProc: Some(wnd_proc),
            hbrBackground: HBRUSH(0),
            ..Default::default()
        };

        // SAFETY: the class name and window procedure are defined in this
        // module and outlive the window.
        unsafe {
            if RegisterClassW(&raw const wnd_class) == 0 {
                return;
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                PCWSTR::null(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                instance,
                None,
            );
            if hwnd.0 == 0 {
                return;
            }
        }

        let mut message = MSG::default();
        // SAFETY: standard Win32 message loop; GetMessageW blocks until a
        // message arrives and returns 0 on WM_QUIT.
        unsafe {
            loop {
                let result = GetMessageW(&raw mut message, None, 0, 0);
                if result.0 <= 0 {
                    return;
                }
                TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }

    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_DEVICECHANGE => {
                if matches!(
                    wparam.0,
                    DBT_DEVNODES_CHANGED | DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE
                ) && let Some(tx) = WAKE.get()
                {
                    // Debounce and settle-delay live in the watcher loop, so
                    // this handler stays cheap and never blocks the pump.
                    let _ = tx.send(());
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // SAFETY: valid during window destruction.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            // SAFETY: the documented fallback for unhandled messages.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
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
