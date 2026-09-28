use dioxus::prelude::*;

use crate::app::tool_path;
use crate::config::MonitorRule;
use crate::hardware::{self, MonitorInfo};
use crate::ui::{AppState, Handles};

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
    let mut refresh = use_signal(|| 0u32);
    let mut selected = use_signal(|| 0usize);

    let dir = handles.dir.clone();
    let detected = use_memo(move || {
        let _ = refresh();
        let cfg = config();
        let tool = tool_path(&cfg, &dir);
        hardware::list_monitors(&tool).unwrap_or_default()
    });

    let monitors = detected();
    let boxes = scale_layout(&monitors, 700.0, 280.0); // matches .layout in style.css

    rsx! {
        h1 { "Monitors" }
        p { style: "color:var(--text-dim); margin-top:-8px;",
            "Click a screen to choose which input it should show when the KVM is
             connected and when it is not."
        }

        div { style: "display:flex; gap:8px; margin-bottom:12px;",
            button { class: "secondary", onclick: move |_| refresh += 1, "Refresh" }
        }

        div { class: "layout",
            if monitors.is_empty() {
                div { style: "padding:20px; color:var(--text-dim)",
                    "No monitors detected. Check ControlMyMonitor.exe in Settings."
                }
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
            MonitorPanel { monitor: mon.clone() }
        }

        {
            let cfg = config();
            let unmatched: Vec<MonitorRule> = cfg
                .monitors
                .iter()
                .filter(|r| r.monitor_id.is_empty() || !monitors.iter().any(|m| m.monitor_id == r.monitor_id))
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

// ponytail: stub until Task 14 builds the input-rule panel.
#[component]
fn MonitorPanel(monitor: MonitorInfo) -> Element {
    let _ = monitor;
    rsx! { div {} }
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
