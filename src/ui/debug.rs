//! Simulation controls for developing on Linux, where no WM_DEVICECHANGE
//! exists. Events are pushed through the same channel the Windows message
//! pump uses, so the code path under test is the real one.

use dioxus::prelude::*;

use crate::hardware;
use crate::ui::{AppState, Handles};

#[component]
pub fn Debug() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let poke = {
        let handles = handles.clone();
        move || {
            if let Some(wake) = &handles.wake {
                let _ = wake.send(());
            }
        }
    };

    rsx! {
        h1 { "Debug" }
        div { class: "banner warn",
            "This panel exists only on non-Windows builds. It simulates the device
             notifications Windows would deliver."
        }

        div { class: "card",
            h2 { "KVM simulation" }
            div { style: "display:flex; gap:8px; flex-wrap:wrap;",
                button { class: "primary",
                    onclick: {
                        let poke = poke.clone();
                        move |_| { hardware::set_all_present(true); poke(); }
                    },
                    "Plug everything in"
                }
                button { class: "secondary",
                    onclick: {
                        let poke = poke.clone();
                        move |_| { hardware::set_all_present(false); poke(); }
                    },
                    "Unplug everything"
                }
                button { class: "secondary",
                    onclick: {
                        let poke = poke.clone();
                        move |_| {
                            // Rapid toggle: exercises the 500ms settle window
                            // and the cooldown suppression path.
                            for i in 0..6 {
                                hardware::set_all_present(i % 2 == 0);
                                poke();
                            }
                        }
                    },
                    "Rapid toggle x6"
                }
            }
        }

        div { class: "card",
            h2 { "Individual devices" }
            table {
                thead { tr { th { "Device" } th { "ID" } th { "" } } }
                tbody {
                    for d in hardware::list_devices().unwrap_or_default() {
                        tr {
                            td { "{d.name}" }
                            td { "{d.id}" }
                            td {
                                button { class: "secondary",
                                    onclick: {
                                        let poke = poke.clone();
                                        let id = d.id.clone();
                                        move |_| { hardware::set_device_present(&id, false); poke(); }
                                    },
                                    "Unplug"
                                }
                            }
                        }
                    }
                }
            }
        }

        div { class: "card",
            h2 { "Failure injection" }
            div { style: "display:flex; gap:8px; flex-wrap:wrap;",
                button { class: "secondary",
                    onclick: move |_| {
                        hardware::set_tool_present(false);
                        state.tool_ok.clone().set(false);
                    },
                    "Hide ControlMyMonitor.exe"
                }
                button { class: "secondary",
                    onclick: move |_| {
                        hardware::set_tool_present(true);
                        state.tool_ok.clone().set(true);
                    },
                    "Restore ControlMyMonitor.exe"
                }
                button { class: "secondary",
                    onclick: move |_| hardware::set_fail_next_command(true),
                    "Fail the next switch"
                }
                button { class: "secondary",
                    onclick: move |_| {
                        let mut c = (state.config)();
                        c.monitors.push(crate::config::MonitorRule {
                            serial: String::new(),
                            label: "Unmatched monitor".into(),
                            on_connect: 15,
                            on_disconnect: 17,
                        });
                        handles.save(&c);
                        state.config.clone().set(c);
                    },
                    "Add an unmatched monitor rule"
                }
            }
        }
    }
}
