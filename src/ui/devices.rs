use dioxus::prelude::*;

use crate::config::{Config, DeviceEntry};
use crate::hardware::{self, DeviceClass, UsbDevice};
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

/// Devices present in exactly one of the two lists — what came or went.
fn changed(before: &[UsbDevice], after: &[UsbDevice]) -> Vec<UsbDevice> {
    let missing_from = |list: &[UsbDevice], d: &UsbDevice| !list.iter().any(|x| x.id == d.id);
    before
        .iter()
        .filter(|d| missing_from(after, d))
        .chain(after.iter().filter(|d| missing_from(before, d)))
        .cloned()
        .collect()
}

fn set_watched(mut config: Signal<Config>, handles: &Handles, d: &UsbDevice, on: bool) {
    let mut c = config();
    if on {
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

/// User label, else the watched entry's cached name, else the hardware name.
fn display_name(cfg: &Config, id: &str, hw_name: &str) -> String {
    cfg.labels
        .get(id)
        .or_else(|| cfg.devices.iter().find(|e| e.id == id).map(|e| &e.name))
        .map_or_else(|| hw_name.to_string(), String::clone)
}

/// An editable device name. Clearing it restores the hardware name.
#[component]
fn NameCell(id: String, hw_name: String) -> Element {
    let mut config = use_context::<AppState>().config;
    let handles = use_context::<Handles>();
    let shown = display_name(&config(), &id, &hw_name);
    rsx! {
        input {
            r#type: "text",
            class: "rename",
            value: "{shown}",
            title: "{hw_name}",
            style: "width:100%",
            onchange: move |e: Event<FormData>| {
                let mut c = config();
                let label = e.value().trim().to_string();
                if label.is_empty() {
                    c.labels.remove(&id);
                    // Also undo a name stored before labels existed.
                    if let Some(entry) = c.devices.iter_mut().find(|x| x.id == id) {
                        entry.name = hw_name.clone();
                    }
                } else {
                    c.labels.insert(id.clone(), label);
                }
                handles.save(&c);
                config.set(c);
            },
        }
    }
}

#[component]
pub fn Devices() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let config = state.config;
    let devices_rev = state.devices_rev;
    let mut query = use_signal(String::new);
    let mut only_watched = use_signal(|| false);
    let mut refresh = use_signal(|| 0u32);
    // Some(snapshot) while detecting; `found` accumulates everything that
    // differs from the snapshot, so a KVM toggle arriving in several bursts
    // is still caught whole.
    let mut detect = use_signal(|| None::<Vec<UsbDevice>>);
    let mut found = use_signal(Vec::<UsbDevice>::new);

    // Re-enumerates on Refresh and on every USB change the watcher reports.
    let present_memo = use_memo(move || {
        let _ = refresh();
        let _ = devices_rev();
        hardware::list_devices().unwrap_or_default()
    });

    use_effect(move || {
        let now = present_memo();
        if let Some(snapshot) = detect() {
            found.with_mut(|f| {
                for d in changed(&snapshot, &now) {
                    if !f.iter().any(|x| x.id == d.id) {
                        f.push(d);
                    }
                }
            });
        }
    });

    let cfg = config();
    let needle = query().to_lowercase();
    let present = present_memo();
    let label = |d: &UsbDevice| display_name(&cfg, &d.id, &d.name);

    // Configured devices that are not currently plugged in.
    let absent: Vec<_> = cfg
        .devices
        .iter()
        .filter(|e| !present.iter().any(|d| d.id == e.id))
        .cloned()
        .collect();

    let visible: Vec<_> = present
        .iter()
        .filter(|d| {
            (!only_watched() || cfg.watches(&d.id))
                && (needle.is_empty()
                    || label(d).to_lowercase().contains(&needle)
                    || d.name.to_lowercase().contains(&needle)
                    || d.id.to_lowercase().contains(&needle))
        })
        .cloned()
        .collect();

    rsx! {
        h1 { "Devices" }
        p { style: "color:var(--text-dim); margin-top:-8px;",
            "Switch on the devices that move with your KVM. When all of them
             disappear or all come back, your monitors follow. Click any device's name to rename it."
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
            if detect().is_none() {
                button { class: "primary",
                    onclick: move |_| {
                        found.set(Vec::new());
                        detect.set(Some(present_memo()));
                    },
                    "Detect"
                }
            }
        }

        if detect().is_some() {
            div { class: "card",
                div { style: "display:flex; justify-content:space-between; align-items:center;",
                    span {
                        if found().is_empty() {
                            "Detecting… unplug or plug in a device, or toggle your KVM."
                        } else {
                            "Detected — keep going, or click Done."
                        }
                    }
                    button { class: "secondary", onclick: move |_| detect.set(None), "Done" }
                }
                if !found().is_empty() {
                    table {
                        tbody {
                            for d in found() {
                                tr { key: "{d.id}",
                                    td { "{icon(d.class)}" }
                                    td { NameCell { id: d.id.clone(), hw_name: d.name.clone() } }
                                    td { style: "color:var(--text-dim); font-family:monospace", "{d.id}" }
                                    td {
                                        input {
                                            r#type: "checkbox",
                                            checked: cfg.watches(&d.id),
                                            onchange: {
                                                let handles = handles.clone();
                                                move |e: Event<FormData>| set_watched(config, &handles, &d, e.checked())
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
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
                            td { NameCell { id: d.id.clone(), hw_name: d.name.clone() } }
                            td { style: "color:var(--text-dim); font-family:monospace", "{d.id}" }
                            td {
                                input {
                                    r#type: "checkbox",
                                    checked: cfg.watches(&d.id),
                                    onchange: {
                                        let handles = handles.clone();
                                        move |e: Event<FormData>| set_watched(config, &handles, &d, e.checked())
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
                                td { NameCell { id: e.id.clone(), hw_name: e.name.clone() } }
                                td { style: "font-family:monospace", "{e.id}" }
                                td {
                                    button { class: "secondary",
                                        onclick: {
                                            let id = e.id.clone();
                                            let handles = handles.clone();
                                            let mut config = config;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str) -> UsbDevice {
        UsbDevice { id: id.into(), name: id.into(), class: DeviceClass::Other }
    }

    #[test]
    fn changed_reports_devices_that_came_and_went() {
        let before = [dev("A"), dev("B")];
        let after = [dev("B"), dev("C")];
        let ids: Vec<_> = changed(&before, &after).into_iter().map(|d| d.id).collect();
        assert_eq!(ids, ["A", "C"]);
        assert!(changed(&before, &before).is_empty());
    }

    #[test]
    fn labels_win_over_cached_and_hardware_names_and_survive_toml() {
        let mut cfg = Config::default();
        let id = "VID_1E7D&PID_2CB2";
        assert_eq!(display_name(&cfg, id, "USB Input Device"), "USB Input Device");

        cfg.devices.push(DeviceEntry { id: id.into(), name: "Cached".into(), class: "HIDClass".into() });
        assert_eq!(display_name(&cfg, id, "USB Input Device"), "Cached");

        cfg.labels.insert(id.into(), "KVM Mouse".into());
        let cfg: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(display_name(&cfg, id, "USB Input Device"), "KVM Mouse");
    }
}
