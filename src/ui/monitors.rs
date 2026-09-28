use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dioxus::prelude::*;
use futures_channel::mpsc::UnboundedReceiver;
use futures_util::StreamExt;

use crate::app::tool_path;
use crate::config::MonitorRule;
use crate::hardware::{self, MonitorInfo};
use crate::ui::{off_thread, scan_monitors, AppState, Handles};

/// Standard DDC/CI input-select values. Vendors deviate, which is why the
/// custom field and the Test button exist.
pub const INPUT_PRESETS: &[(u16, &str)] = &[
    (1, "VGA"),
    (3, "DVI-1"),
    (4, "DVI-2"),
    (15, "DisplayPort 1"),
    (16, "DisplayPort 2"),
    (17, "HDMI 1"),
    (18, "HDMI 2"),
];

pub fn input_label(value: u16) -> String {
    INPUT_PRESETS
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(v, name)| format!("{name} ({v})"))
        .unwrap_or_else(|| format!("Custom ({value})"))
}

/// Projects virtual-desktop rectangles onto a canvas of `cw` x `ch` pixels.
///
/// Returns `(index, left, top, width, height)` per monitor. A single uniform
/// scale is used so relative sizes and gaps survive the projection.
pub fn scale_layout(
    monitors: &[MonitorInfo],
    cw: f64,
    ch: f64,
) -> Vec<(usize, f64, f64, f64, f64)> {
    if monitors.is_empty() {
        return Vec::new();
    }

    let min_x = monitors.iter().map(|m| m.x).min().unwrap_or(0) as f64;
    let min_y = monitors.iter().map(|m| m.y).min().unwrap_or(0) as f64;
    let max_x = monitors.iter().map(|m| m.x + m.width).max().unwrap_or(1) as f64;
    let max_y = monitors.iter().map(|m| m.y + m.height).max().unwrap_or(1) as f64;

    let span_x = (max_x - min_x).max(1.0);
    let span_y = (max_y - min_y).max(1.0);

    const PAD: f64 = 16.0;
    let scale = ((cw - PAD * 2.0) / span_x).min((ch - PAD * 2.0) / span_y);

    // Centre the whole arrangement in the canvas.
    let off_x = (cw - span_x * scale) / 2.0;
    let off_y = (ch - span_y * scale) / 2.0;

    monitors
        .iter()
        .enumerate()
        .map(|(i, m)| {
            (
                i,
                (m.x as f64 - min_x) * scale + off_x,
                (m.y as f64 - min_y) * scale + off_y,
                m.width as f64 * scale,
                m.height as f64 * scale,
            )
        })
        .collect()
}

#[component]
pub fn Monitors() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let mut selected = use_signal(|| 0usize);

    // Scanned at launch (see `ui::scan_monitors`), so opening this view is instant.
    let scan = (state.monitors)();
    let monitors = match &scan {
        Some(Ok(m)) => m.clone(),
        _ => Vec::new(),
    };
    let status = match &scan {
        None => Some("Scanning monitors…".to_string()),
        Some(Err(e)) => Some(format!("Could not list monitors: {e}")),
        Some(Ok(m)) if m.is_empty() => Some("No DDC/CI monitors found.".to_string()),
        Some(Ok(_)) => None,
    };
    let boxes = scale_layout(&monitors, 700.0, 280.0); // matches .layout in style.css

    rsx! {
        h1 { "Monitors" }
        p { style: "color:var(--text-dim); margin-top:-8px;",
            "Click a screen to choose which input it should show when the KVM is
             connected and when it is not."
        }

        div { style: "display:flex; gap:8px; margin-bottom:12px;",
            button { class: "secondary",
                disabled: scan.is_none(),
                onclick: {
                    let dir = handles.dir.clone();
                    move |_| scan_monitors(state.monitors, tool_path(&config(), &dir))
                },
                "Refresh"
            }
        }

        div { class: "layout",
            if let Some(status) = status {
                div { style: "padding:20px; color:var(--text-dim)", "{status}" }
            }
            for (i, left, top, w, h) in boxes {
                div {
                    key: "{monitors[i].monitor_id}",
                    class: if selected() == i { "screen selected" } else { "screen" },
                    style: "left:{left}px; top:{top}px; width:{w}px; height:{h}px;",
                    onclick: move |_| selected.set(i),
                    div { class: "num", "{i + 1}" }
                    div { class: "meta", "{monitors[i].model}" }
                    div { class: "meta",
                        {monitors[i].current_input.map(input_label).unwrap_or_else(|| "input ?".into())}
                    }
                }
            }
        }

        if let Some(mon) = monitors.get(selected()) {
            // Keyed so a test running on one monitor doesn't follow the selection.
            MonitorPanel { key: "{mon.monitor_id}", monitor: mon.clone() }
        }

        {
            let cfg = config();
            // Mid-scan the list is empty; don't flash every rule as unmatched.
            let scanned = matches!(scan, Some(Ok(_)));
            let unmatched: Vec<MonitorRule> = cfg
                .monitors
                .iter()
                .filter(|r| scanned && !monitors.iter().any(|m| m.monitor_id == r.monitor_id))
                .cloned()
                .collect();

            rsx! {
                if !unmatched.is_empty() {
                    h2 { "Unmatched rules" }
                    div { class: "card",
                        p { style: "color:var(--text-dim)",
                            "These rules reference monitors that are not currently detected.
                             They are kept so nothing is lost, but they will be skipped."
                        }
                        table {
                            tbody {
                                for r in unmatched {
                                    tr { key: "{r.label}-{r.on_connect}",
                                        td { "{r.label}" }
                                        td { "connected → {input_label(r.on_connect)}" }
                                        td { "disconnected → {input_label(r.on_disconnect)}" }
                                        td {
                                            button { class: "secondary",
                                                onclick: {
                                                    let r = r.clone();
                                                    let handles = handles.clone();
                                                    move |_| {
                                                        let mut c = config();
                                                        c.monitors.retain(|x| {
                                                            !(x.monitor_id == r.monitor_id && x.label == r.label)
                                                        });
                                                        handles.save(&c);
                                                        config.set(c);
                                                    }
                                                },
                                                "Remove"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
enum TestPhase {
    Idle,
    /// Showing the candidate, counting down before the automatic revert.
    Running { candidate: u16, seconds_left: u32 },
    /// Reverted; waiting for the user to say what to do with the candidate.
    Asking { candidate: u16 },
    Failed { message: String },
}

/// Long enough for the monitor to resync and a sleeping PC on the other
/// input to wake; "+10 s" and "Switch back now" adjust it mid-test.
const TEST_SECONDS: u32 = 15;
const EXTEND_SECONDS: u32 = 10;

enum TestMsg {
    Tick(u32),
    Done(Result<(), String>),
}

/// Runs a whole input test on its own thread: switch, count `left` down to
/// zero, switch back. The switch-back lives here, not in the UI, so closing
/// the panel mid-test can't strand the monitor on the test input.
fn run_test(tool: PathBuf, id: String, target: u16, left: Arc<AtomicU32>) -> UnboundedReceiver<TestMsg> {
    let (tx, rx) = futures_channel::mpsc::unbounded();
    std::thread::spawn(move || {
        let switched = hardware::read_input(&tool, &id)
            .and_then(|previous| hardware::apply_input(&tool, &id, target).map(|()| previous));
        let previous = match switched {
            Ok(p) => p,
            Err(e) => {
                let _ = tx.unbounded_send(TestMsg::Done(Err(e.to_string())));
                return;
            }
        };
        loop {
            let l = left.load(Ordering::Relaxed);
            let _ = tx.unbounded_send(TestMsg::Tick(l));
            if l == 0 {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
            let _ = left.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1));
        }
        let back = hardware::apply_input(&tool, &id, previous)
            .map_err(|e| format!("could not switch back: {e}"));
        let _ = tx.unbounded_send(TestMsg::Done(back));
    });
    rx
}

/// Input rules for one monitor, plus a test that reverts *before* asking,
/// since mid-test this window may be on an input the user can't see.
#[component]
fn MonitorPanel(monitor: MonitorInfo) -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let mut phase = use_signal(|| TestPhase::Idle);
    let mut candidate = use_signal(|| 15u16);
    // Seconds until switch-back, shared with the test thread.
    let left = use_hook(|| Arc::new(AtomicU32::new(0)));

    let cfg = config();
    let saved = cfg.monitors.iter().find(|r| r.monitor_id == monitor.monitor_id).cloned();
    let rule = saved.clone().unwrap_or(MonitorRule {
        monitor_id: monitor.monitor_id.clone(),
        label: monitor.model.clone(),
        on_connect: monitor.current_input.unwrap_or(15),
        on_disconnect: monitor.current_input.unwrap_or(17),
    });

    // Writes a rule field, creating the rule if this monitor has none yet.
    let write_rule = {
        let handles = handles.clone();
        let rule = rule.clone();
        move |on_connect: Option<u16>, on_disconnect: Option<u16>| {
            let mut c = config();
            let mut r = c
                .monitors
                .iter()
                .find(|x| x.monitor_id == rule.monitor_id)
                .cloned()
                .unwrap_or_else(|| rule.clone());
            r.on_connect = on_connect.unwrap_or(r.on_connect);
            r.on_disconnect = on_disconnect.unwrap_or(r.on_disconnect);
            c.monitors.retain(|x| x.monitor_id != r.monitor_id);
            c.monitors.push(r);
            handles.save(&c);
            config.set(c);
        }
    };

    let cooling_down = (state.cooldown)().is_some();

    // Ask the monitor which inputs it has, once per session per monitor.
    let mut cache = state.supported_inputs;
    use_hook({
        let dir = handles.dir.clone();
        let id = monitor.monitor_id.clone();
        move || {
            if cache.peek().contains_key(&id) {
                return;
            }
            let tool = tool_path(&config.peek(), &dir);
            spawn(async move {
                let inputs = off_thread({
                    let id = id.clone();
                    move || hardware::supported_inputs(&tool, &id).unwrap_or_default()
                })
                .await;
                cache.write().insert(id, inputs);
            });
        }
    });
    let reported = cache().get(&monitor.monitor_id).cloned();
    let loading = reported.is_none();
    let unreported = reported.as_ref().is_some_and(Vec::is_empty);
    let options: Vec<u16> = match reported {
        Some(inputs) if !inputs.is_empty() => inputs,
        _ => INPUT_PRESETS.iter().map(|(v, _)| *v).collect(),
    };
    // The select can only show a listed value, so test what it shows.
    let target = if options.contains(&candidate()) { candidate() } else { options[0] };

    rsx! {
        div { class: "card",
            h2 { "{monitor.model}" }
            p { style: "color:var(--text-dim); margin-top:-4px; font-family:monospace; font-size:12px;",
                "{monitor.device_name} · {monitor.width}×{monitor.height}"
                if monitor.is_primary { " · primary" }
            }

            div { style: "display:flex; gap:24px; flex-wrap:wrap; margin-top:12px;",
                InputChooser {
                    label: "When the KVM is connected",
                    options: options.clone(),
                    value: rule.on_connect,
                    on_change: {
                        let mut write_rule = write_rule.clone();
                        move |v| write_rule(Some(v), None)
                    },
                }
                InputChooser {
                    label: "When the KVM is disconnected",
                    options: options.clone(),
                    value: rule.on_disconnect,
                    on_change: {
                        let mut write_rule = write_rule.clone();
                        move |v| write_rule(None, Some(v))
                    },
                }
            }

            div { style: "margin-top:12px; color:var(--text-dim);",
                if saved.is_some() {
                    button { class: "secondary",
                        onclick: {
                            let handles = handles.clone();
                            let id = monitor.monitor_id.clone();
                            move |_| {
                                let mut c = config();
                                c.monitors.retain(|x| x.monitor_id != id);
                                handles.save(&c);
                                config.set(c);
                            }
                        },
                        "Stop switching this monitor"
                    }
                } else {
                    "Not switching yet — pick an input above to start."
                }
            }
        }

        div { class: "card",
            h2 { "Test an input" }
            p { style: "color:var(--text-dim); margin-top:-4px;",
                "The monitor switches to the chosen input for {TEST_SECONDS} seconds, then
                 switches back on its own. You are asked what to do with it afterwards, so
                 you are never stranded on an input you cannot see."
            }
            if loading {
                p { style: "color:var(--text-dim);", "Reading this monitor's inputs… (common inputs listed meanwhile)" }
            }
            if unreported {
                p { style: "color:var(--text-dim);",
                    "This monitor didn't report its inputs, so the common ones are listed. Typical values:
                     VGA = 1, DVI = 3, DisplayPort 1 = 15, DisplayPort 2 = 16, HDMI 1 = 17, HDMI 2 = 18."
                }
            }

            match phase() {
                TestPhase::Idle => rsx! {
                    div { style: "display:flex; gap:10px; align-items:center;",
                        select {
                            onchange: move |e| {
                                if let Ok(v) = e.value().parse::<u16>() { candidate.set(v) }
                            },
                            for v in options.clone() {
                                option { value: "{v}", selected: v == target, "{input_label(v)}" }
                            }
                        }
                        button {
                            class: "primary",
                            disabled: cooling_down,
                            onclick: {
                                let dir = handles.dir.clone();
                                let id = monitor.monitor_id.clone();
                                let left = left.clone();
                                move |_| {
                                    left.store(TEST_SECONDS, Ordering::Relaxed);
                                    phase.set(TestPhase::Running { candidate: target, seconds_left: TEST_SECONDS });
                                    let mut rx = run_test(tool_path(&config(), &dir), id.clone(), target, left.clone());
                                    spawn(async move {
                                        while let Some(msg) = rx.next().await {
                                            phase.set(match msg {
                                                TestMsg::Tick(seconds_left) => TestPhase::Running { candidate: target, seconds_left },
                                                TestMsg::Done(Ok(())) => TestPhase::Asking { candidate: target },
                                                TestMsg::Done(Err(message)) => TestPhase::Failed { message },
                                            });
                                        }
                                    });
                                }
                            },
                            "Test"
                        }
                        if cooling_down {
                            span { style: "color:var(--text-dim)",
                                "Testing is paused while a switch cooldown is active."
                            }
                        }
                    }
                },

                TestPhase::Running { candidate, seconds_left } => rsx! {
                    div { class: "banner warn",
                        "Showing {input_label(candidate)} — switching back in {seconds_left}s."
                    }
                    div { style: "display:flex; gap:8px;",
                        button { class: "secondary",
                            onclick: {
                                let left = left.clone();
                                move |_| { left.fetch_add(EXTEND_SECONDS, Ordering::Relaxed); }
                            },
                            "+{EXTEND_SECONDS} s"
                        }
                        button { class: "secondary",
                            onclick: {
                                let left = left.clone();
                                move |_| left.store(0, Ordering::Relaxed)
                            },
                            "Switch back now"
                        }
                    }
                },

                TestPhase::Asking { candidate } => rsx! {
                    div { class: "card", style: "background:var(--bg-raised)",
                        p { "Did {input_label(candidate)} show the right source?" }
                        div { style: "display:flex; gap:8px; flex-wrap:wrap;",
                            button { class: "primary",
                                onclick: {
                                    let mut write_rule = write_rule.clone();
                                    move |_| { write_rule(Some(candidate), None); phase.set(TestPhase::Idle); }
                                },
                                "Use when KVM connected"
                            }
                            button { class: "primary",
                                onclick: {
                                    let mut write_rule = write_rule.clone();
                                    move |_| { write_rule(None, Some(candidate)); phase.set(TestPhase::Idle); }
                                },
                                "Use when KVM disconnected"
                            }
                            button { class: "secondary",
                                onclick: move |_| phase.set(TestPhase::Idle),
                                "Discard"
                            }
                        }
                    }
                },

                TestPhase::Failed { message } => rsx! {
                    div { class: "banner err", "Test failed: {message}" }
                    button { class: "secondary", onclick: move |_| phase.set(TestPhase::Idle), "Back" }
                },
            }
        }
    }
}

#[component]
fn InputChooser(label: String, options: Vec<u16>, value: u16, on_change: EventHandler<u16>) -> Element {
    rsx! {
        div {
            div { style: "color:var(--text-dim); margin-bottom:6px;", "{label}" }
            select {
                onchange: move |e| {
                    if let Ok(v) = e.value().parse::<u16>() { on_change.call(v) }
                },
                for v in options.iter().copied() {
                    option { value: "{v}", selected: v == value, "{input_label(v)}" }
                }
                // A saved value the monitor didn't list stays visible rather
                // than silently showing as the first option.
                if !options.contains(&value) {
                    option { value: "{value}", selected: true, "{input_label(value)} — not listed by monitor" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::MonitorInfo;

    fn m(x: i32, y: i32, w: i32, h: i32) -> MonitorInfo {
        MonitorInfo {
            monitor_id: format!("S{x}"),
            model: "Test".into(),
            device_name: "dev".into(),
            x, y, width: w, height: h,
            is_primary: false,
            current_input: None,
        }
    }

    #[test]
    fn layout_fits_inside_the_canvas_and_preserves_relative_position() {
        let monitors = vec![m(0, 0, 3840, 2160), m(3840, -400, 1080, 1920)];
        let boxes = scale_layout(&monitors, 800.0, 280.0);

        assert_eq!(boxes.len(), 2);
        for (_, left, top, w, h) in &boxes {
            assert!(*left >= 0.0 && *top >= 0.0);
            assert!(left + w <= 800.5, "box overflows the canvas width");
            assert!(top + h <= 280.5, "box overflows the canvas height");
        }
        // The second monitor sits to the right of and above the first.
        assert!(boxes[1].1 > boxes[0].1);
        assert!(boxes[1].2 < boxes[0].2);
    }

    #[test]
    fn aspect_ratio_is_preserved() {
        let monitors = vec![m(0, 0, 1920, 1080)];
        let (_, _, _, w, h) = scale_layout(&monitors, 800.0, 280.0)[0];
        assert!(((w / h) - (1920.0 / 1080.0)).abs() < 0.01);
    }

    #[test]
    fn an_empty_list_yields_no_boxes() {
        assert!(scale_layout(&[], 800.0, 280.0).is_empty());
    }
}
