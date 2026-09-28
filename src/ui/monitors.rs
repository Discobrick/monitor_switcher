use dioxus::prelude::*;

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
    Running { candidate: u16, seconds_left: u8 },
    /// Reverted; waiting for the user to say what to do with the candidate.
    Asking { candidate: u16 },
    Failed { message: String },
}

const TEST_SECONDS: u8 = 8;

/// Input rules for one monitor, plus a test that reverts *before* asking,
/// since mid-test this window may be on an input the user can't see.
#[component]
fn MonitorPanel(monitor: MonitorInfo) -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let mut phase = use_signal(|| TestPhase::Idle);
    let mut candidate = use_signal(|| 15u16);

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
                    value: rule.on_connect,
                    on_change: {
                        let mut write_rule = write_rule.clone();
                        move |v| write_rule(Some(v), None)
                    },
                }
                InputChooser {
                    label: "When the KVM is disconnected",
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

            match phase() {
                TestPhase::Idle => rsx! {
                    div { style: "display:flex; gap:10px; align-items:center;",
                        select {
                            value: "{candidate}",
                            onchange: move |e| {
                                if let Ok(v) = e.value().parse::<u16>() { candidate.set(v) }
                            },
                            for (v, name) in INPUT_PRESETS {
                                option { value: "{v}", "{name} ({v})" }
                            }
                        }
                        input {
                            r#type: "number", min: "1", max: "255",
                            style: "width:90px",
                            value: "{candidate}",
                            oninput: move |e| {
                                if let Ok(v) = e.value().parse::<u16>() { candidate.set(v) }
                            },
                        }
                        button {
                            class: "primary",
                            disabled: cooling_down,
                            onclick: {
                                let dir = handles.dir.clone();
                                let id = monitor.monitor_id.clone();
                                move |_| {
                                    let tool = tool_path(&config(), &dir);
                                    let id = id.clone();
                                    let target = candidate();
                                    phase.set(TestPhase::Running { candidate: target, seconds_left: TEST_SECONDS });
                                    spawn(async move {
                                        let switched = {
                                            let (tool, id) = (tool.clone(), id.clone());
                                            off_thread(move || {
                                                let previous = hardware::read_input(&tool, &id)?;
                                                hardware::apply_input(&tool, &id, target)?;
                                                Ok::<_, hardware::HardwareError>(previous)
                                            })
                                            .await
                                        };
                                        let previous = match switched {
                                            Ok(p) => p,
                                            Err(e) => {
                                                phase.set(TestPhase::Failed { message: e.to_string() });
                                                return;
                                            }
                                        };

                                        // Count down, then revert unconditionally.
                                        for seconds_left in (0..TEST_SECONDS).rev() {
                                            off_thread(|| std::thread::sleep(std::time::Duration::from_secs(1))).await;
                                            phase.set(TestPhase::Running { candidate: target, seconds_left });
                                        }
                                        match off_thread(move || hardware::apply_input(&tool, &id, previous)).await {
                                            Ok(()) => phase.set(TestPhase::Asking { candidate: target }),
                                            Err(e) => phase.set(TestPhase::Failed {
                                                message: format!("could not switch back: {e}"),
                                            }),
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
fn InputChooser(label: String, value: u16, on_change: EventHandler<u16>) -> Element {
    rsx! {
        div {
            div { style: "color:var(--text-dim); margin-bottom:6px;", "{label}" }
            select {
                value: "{value}",
                onchange: move |e| {
                    if let Ok(v) = e.value().parse::<u16>() { on_change.call(v) }
                },
                for (v, name) in INPUT_PRESETS {
                    option { value: "{v}", selected: *v == value, "{name} ({v})" }
                }
                if !INPUT_PRESETS.iter().any(|(v, _)| *v == value) {
                    option { value: "{value}", selected: true, "Custom ({value})" }
                }
            }
            input {
                r#type: "number", min: "1", max: "255",
                style: "width:90px; margin-left:8px;",
                value: "{value}",
                // onchange, not oninput: save once per edit, not per keystroke.
                onchange: move |e| {
                    if let Ok(v) = e.value().parse::<u16>() { on_change.call(v) }
                },
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
