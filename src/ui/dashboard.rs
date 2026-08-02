use dioxus::prelude::*;

use crate::app::{Command, Severity};
use crate::ui::{AppState, Handles};

/// Pill CSS class and label for the KVM presence indicator. Pure so it's
/// reachable by `cargo test` without a running component tree.
fn presence_pill(present: bool) -> (&'static str, &'static str) {
    if present {
        ("pill on", "KVM Connected")
    } else {
        ("pill off", "KVM Disconnected")
    }
}

/// CSS class for a log line, keyed off its severity.
fn severity_class(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Success => "success",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

#[component]
pub fn Dashboard() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let present = (state.present)();
    let cooldown = (state.cooldown)();
    let enabled = config().monitoring_enabled;
    let (pill_class, pill_label) = presence_pill(present);

    rsx! {
        h1 { "Dashboard" }

        if !(state.tool_ok)() {
            div { class: "banner err",
                "ControlMyMonitor.exe was not found. Monitoring is paused — set it up in Settings."
            }
        }

        div { class: "card",
            div { class: pill_class, {pill_label} }

            div { style: "margin-top:14px; display:flex; align-items:center; gap:14px;",
                button {
                    class: if enabled { "secondary" } else { "primary" },
                    onclick: move |_| {
                        let mut c = config();
                        c.monitoring_enabled = !c.monitoring_enabled;
                        let _ = handles.commands.send(Command::SetMonitoring(c.monitoring_enabled));
                        handles.save(&c);
                        config.set(c);
                    },
                    if enabled { "Pause monitoring" } else { "Resume monitoring" }
                }

                if let Some(secs) = cooldown {
                    span { style: "color:var(--text-dim)",
                        "Cooldown: {secs}s remaining — switches are suppressed"
                    }
                }
            }
        }

        h2 { "Activity" }
        div { class: "card log",
            if (state.log)().is_empty() {
                div { class: "info", "Nothing yet. Toggle your KVM to see events here." }
            }
            for entry in (state.log)().iter().rev() {
                div {
                    class: severity_class(entry.severity),
                    "[{entry.at}] {entry.message}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_pill_on_when_present() {
        assert_eq!(presence_pill(true), ("pill on", "KVM Connected"));
    }

    #[test]
    fn presence_pill_off_when_absent() {
        assert_eq!(presence_pill(false), ("pill off", "KVM Disconnected"));
    }

    #[test]
    fn severity_class_maps_each_variant() {
        assert_eq!(severity_class(Severity::Info), "info");
        assert_eq!(severity_class(Severity::Success), "success");
        assert_eq!(severity_class(Severity::Warning), "warning");
        assert_eq!(severity_class(Severity::Error), "error");
    }
}
