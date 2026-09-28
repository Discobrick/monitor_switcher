use dioxus::prelude::*;

use crate::app::tool_path;
use crate::deps;
use crate::startup;
use crate::ui::{off_thread, scan_monitors, AppState, Handles};

#[component]
pub fn Settings() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let mut tool_ok = state.tool_ok;
    let mut busy = use_signal(|| false);
    let mut message = use_signal(String::new);
    let mut at_startup = use_signal(startup::is_enabled);

    let cfg = config();
    let tool = tool_path(&cfg, &handles.dir);
    let found = deps::is_present(&tool);

    // After the tool appears or moves: refresh the dashboard banner and rescan.
    let tool_changed = {
        let dir = handles.dir.clone();
        move || {
            let tool = tool_path(&config(), &dir);
            tool_ok.set(deps::is_present(&tool));
            scan_monitors(state.monitors, tool);
        }
    };

    rsx! {
        h1 { "Settings" }

        div { class: "card",
            h2 { "ControlMyMonitor" }
            p {
                if found {
                    span { style: "color:var(--ok)", "Found: " }
                } else {
                    span { style: "color:var(--err)", "Missing: " }
                }
                span { style: "font-family:monospace; font-size:12px;", "{tool.display()}" }
            }

            if !found {
                button {
                    class: "primary",
                    disabled: busy(),
                    onclick: {
                        let handles = handles.clone();
                        let tool_changed = tool_changed.clone();
                        move |_| {
                            busy.set(true);
                            message.set("Downloading from nirsoft.net…".into());
                            let dir = handles.dir.clone();
                            let handles = handles.clone();
                            let mut tool_changed = tool_changed.clone();
                            spawn(async move {
                                let result = off_thread(move || deps::download_to(&dir)).await;
                                busy.set(false);
                                match result {
                                    Ok(()) => {
                                        message.set(format!("{} installed.", deps::EXE_NAME));
                                        // It landed next to the app; stop looking elsewhere.
                                        config.write().control_my_monitor_path.clear();
                                        handles.save(&config());
                                    }
                                    Err(e) => message.set(format!("Download failed: {e}")),
                                }
                                tool_changed();
                            });
                        }
                    },
                    if busy() { "Downloading…" } else { "Download from NirSoft" }
                }
            }

            div { style: "margin-top:10px;",
                div { style: "color:var(--text-dim); margin-bottom:4px;",
                    "Or point at an existing copy (leave empty to use the one next to the app):"
                }
                input {
                    r#type: "text",
                    style: "width:100%",
                    placeholder: r"C:\Tools\ControlMyMonitor.exe",
                    value: "{cfg.control_my_monitor_path}",
                    onchange: {
                        let handles = handles.clone();
                        let mut tool_changed = tool_changed.clone();
                        move |e: Event<FormData>| {
                            config.write().control_my_monitor_path = e.value().trim().to_string();
                            handles.save(&config());
                            tool_changed();
                        }
                    },
                }
            }

            if !message().is_empty() {
                p { style: "color:var(--text-dim)", "{message}" }
            }

            p { style: "color:var(--text-dim); font-size:12px; margin-bottom:0;",
                "ControlMyMonitor is a free utility by NirSoft — nirsoft.net/utils/control_my_monitor.html"
            }
        }

        div { class: "card",
            h2 { "Behaviour" }

            label { style: "display:flex; gap:8px; align-items:center; margin-bottom:14px;",
                input {
                    r#type: "checkbox",
                    checked: at_startup(),
                    onchange: move |e| {
                        let want = e.checked();
                        match startup::set_enabled(want) {
                            Ok(()) => at_startup.set(want),
                            Err(err) => message.set(format!("Could not change startup: {err}")),
                        }
                    },
                }
                "Run at startup"
            }

            Slider {
                label: "Switch cooldown",
                min: 2, max: 300,
                value: cfg.cooldown_secs as u32,
                on_input: move |v: u32| config.write().cooldown_secs = v as u64,
                on_commit: {
                    let handles = handles.clone();
                    move |_| handles.save(&config())
                },
                "After a switch, further switches are suppressed for this long. If the KVM
                 changes during the cooldown, the monitors are resynced when it ends."
            }

            Slider {
                label: "Input test duration",
                min: 5, max: 60,
                value: cfg.test_secs,
                on_input: move |v: u32| config.write().test_secs = v,
                on_commit: {
                    let handles = handles.clone();
                    move |_| handles.save(&config())
                },
                "How long Monitors → Test shows the chosen input before switching back.
                 Raise it if the other computer is slow to wake or the monitor slow to
                 resync; you can also add time or switch back early during a test."
            }
        }

        div { class: "card",
            h2 { "Files" }
            p { style: "font-family:monospace; font-size:12px; margin-bottom:0;", "{handles.dir.display()}" }
        }
    }
}

/// A seconds slider that follows the drag live and saves once on release.
#[component]
fn Slider(
    label: String,
    min: u32,
    max: u32,
    value: u32,
    on_input: EventHandler<u32>,
    on_commit: EventHandler<()>,
    children: Element,
) -> Element {
    rsx! {
        div {
            div { style: "color:var(--text-dim); margin-bottom:6px;", "{label}: {value}s" }
            input {
                r#type: "range", min: "{min}", max: "{max}", step: "1",
                style: "width:320px",
                value: "{value}",
                oninput: move |e: Event<FormData>| {
                    if let Ok(v) = e.value().parse::<u32>() { on_input.call(v) }
                },
                onchange: move |_| on_commit.call(()),
            }
            p { style: "color:var(--text-dim); font-size:12px;", {children} }
        }
    }
}
