use dioxus::prelude::*;

use crate::ui::{AppState, Handles};

// ponytail: only the Behaviour card so far; Task 15 adds ControlMyMonitor,
// run-at-startup, cooldown and files.
#[component]
pub fn Settings() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let cfg = config();

    rsx! {
        h1 { "Settings" }

        div { class: "card",
            h2 { "Behaviour" }

            div {
                div { style: "color:var(--text-dim); margin-bottom:6px;",
                    "Input test duration: {cfg.test_secs}s"
                }
                input {
                    r#type: "range", min: "5", max: "60", step: "1",
                    style: "width:320px",
                    value: "{cfg.test_secs}",
                    // Follow the slider live, save once on release.
                    oninput: move |e: Event<FormData>| {
                        if let Ok(v) = e.value().parse::<u32>() {
                            config.write().test_secs = v;
                        }
                    },
                    onchange: move |_| handles.save(&config()),
                }
                p { style: "color:var(--text-dim); font-size:12px;",
                    "How long Monitors → Test shows the chosen input before switching back.
                     Raise it if the other computer is slow to wake or the monitor slow to
                     resync; you can also add time or switch back early during a test."
                }
            }
        }
    }
}
