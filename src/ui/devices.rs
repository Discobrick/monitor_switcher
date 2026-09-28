use dioxus::prelude::*;

use crate::config::DeviceEntry;
use crate::hardware::{self, DeviceClass};
use crate::ui::{AppState, Handles};

fn icon(class: DeviceClass) -> &'static str {
    match class {
        DeviceClass::Mouse => "🖱",
        DeviceClass::Keyboard => "⌨",
        DeviceClass::Camera => "📷",
        DeviceClass::Hid => "🎛",
        DeviceClass::Other => "🔌",
    }
}

#[component]
pub fn Devices() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let mut query = use_signal(String::new);
    let mut only_watched = use_signal(|| false);
    let mut refresh = use_signal(|| 0u32);

    // Re-enumerates whenever `refresh` changes.
    let present = use_memo(move || {
        let _ = refresh();
        hardware::list_devices().unwrap_or_default()
    });

    let cfg = config();
    let needle = query().to_lowercase();
    let present = present();

    // Configured devices that are not currently plugged in.
    let absent: Vec<_> = cfg
        .devices
        .iter()
        .filter(|e| !present.iter().any(|d| d.id == e.id))
        .cloned()
        .collect();

    let visible: Vec<_> = present
        .into_iter()
        .filter(|d| {
            (!only_watched() || cfg.watches(&d.id))
                && (needle.is_empty()
                    || d.name.to_lowercase().contains(&needle)
                    || d.id.to_lowercase().contains(&needle))
        })
        .collect();

    rsx! {
        h1 { "Devices" }
        p { style: "color:var(--text-dim); margin-top:-8px;",
            "Switch on the devices that move with your KVM. When any of them appears
             or disappears, your monitors follow."
        }

        div { style: "display:flex; gap:10px; align-items:center; margin-bottom:14px;",
            input {
                r#type: "text",
                placeholder: "Search devices",
                value: "{query}",
                style: "flex:1",
                oninput: move |e| query.set(e.value()),
            }
            label { style: "display:flex; gap:6px; align-items:center; color:var(--text-dim)",
                input {
                    r#type: "checkbox",
                    checked: only_watched(),
                    onchange: move |e| only_watched.set(e.checked()),
                }
                "Watched only"
            }
            button { class: "secondary", onclick: move |_| refresh += 1, "Refresh" }
        }

        div { class: "card",
            table {
                thead {
                    tr { th { "" } th { "Device" } th { "Hardware ID" } th { "Watch" } }
                }
                tbody {
                    for d in visible {
                        tr {
                            key: "{d.id}",
                            td { "{icon(d.class)}" }
                            td { "{d.name}" }
                            td { style: "color:var(--text-dim); font-family:monospace", "{d.id}" }
                            td {
                                input {
                                    r#type: "checkbox",
                                    checked: cfg.watches(&d.id),
                                    onchange: {
                                        let d = d.clone();
                                        let handles = handles.clone();
                                        move |e: Event<FormData>| {
                                            let mut c = config();
                                            if e.checked() {
                                                if !c.watches(&d.id) {
                                                    c.devices.push(DeviceEntry {
                                                        id: d.id.clone(),
                                                        name: d.name.clone(),
                                                        class: d.class.as_str().to_string(),
                                                    });
                                                }
                                            } else {
                                                c.devices.retain(|x| x.id != d.id);
                                            }
                                            handles.save(&c);
                                            config.set(c);
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }
        }

        if !absent.is_empty() {
            h2 { "Watched but not currently connected" }
            div { class: "card",
                table {
                    tbody {
                        for e in absent {
                            tr { class: "absent", key: "{e.id}",
                                td { "{e.name}" }
                                td { style: "font-family:monospace", "{e.id}" }
                                td {
                                    button { class: "secondary",
                                        onclick: {
                                            let id = e.id.clone();
                                            let handles = handles.clone();
                                            move |_| {
                                                let mut c = config();
                                                c.devices.retain(|x| x.id != id);
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
