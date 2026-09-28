mod dashboard;
mod devices;
mod monitors;
mod settings;

#[cfg(not(windows))]
mod debug;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use dioxus::prelude::*;

use crate::app::{tool_path, Command, Event, LogEntry};
use crate::config::Config;
use crate::hardware::{self, MonitorInfo};

const MAX_LOG: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Devices,
    Monitors,
    Settings,
    #[cfg(not(windows))]
    Debug,
}

/// Everything the views read and write, provided via context.
#[derive(Clone, Copy)]
pub struct AppState {
    pub config: Signal<Config>,
    pub log: Signal<Vec<LogEntry>>,
    pub present: Signal<bool>,
    pub cooldown: Signal<Option<u64>>,
    pub tool_ok: Signal<bool>,
    /// Bumped on every USB change so device lists re-enumerate.
    pub devices_rev: Signal<u32>,
    /// Last monitor scan; `None` while one is running. See `scan_monitors`.
    pub monitors: Signal<Option<Result<Vec<MonitorInfo>, String>>>,
}

/// Enumerates monitors off the UI thread (the DDC reads take a few hundred ms)
/// and stores the result in `target`. Runs once at launch and on Refresh.
pub fn scan_monitors(mut target: Signal<Option<Result<Vec<MonitorInfo>, String>>>, tool: PathBuf) {
    target.set(None);
    let (tx, rx) = futures_channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(hardware::list_monitors(&tool).map_err(|e| e.to_string()));
    });
    spawn(async move {
        if let Ok(result) = rx.await {
            target.set(Some(result));
        }
    });
}

/// Shared handles the views need for side effects.
#[derive(Clone)]
pub struct Handles {
    pub dir: PathBuf,
    pub shared_config: Arc<Mutex<Config>>,
    pub commands: Sender<Command>,
    #[cfg(not(windows))]
    pub wake: Option<Sender<()>>,
}

impl Handles {
    /// Persists the UI's config, mirrors it to the watcher, and notifies it.
    pub fn save(&self, cfg: &Config) {
        if let Err(e) = cfg.save(&self.dir) {
            eprintln!("could not save config: {e}");
            return;
        }
        if let Ok(mut shared) = self.shared_config.lock() {
            *shared = cfg.clone();
        }
        let _ = self.commands.send(Command::ConfigChanged);
    }
}

pub fn launch(initial: Config, handles: Handles, events: Receiver<Event>) {
    let props = RootProps { initial, handles, events: Arc::new(Mutex::new(Some(events))) };

    // WindowHides: closing the window hides it instead of exiting the
    // process. Only the tray icon's Quit item (wired in `Root`) actually
    // exits. Documented at
    // https://docs.rs/dioxus-desktop/0.7.10/dioxus_desktop/enum.WindowCloseBehaviour.html
    dioxus::LaunchBuilder::desktop()
        .with_cfg(
            dioxus::desktop::Config::new()
                .with_window(
                    dioxus::desktop::WindowBuilder::new()
                        .with_title("Monitor Switcher")
                        .with_inner_size(dioxus::desktop::LogicalSize::new(980.0, 680.0)),
                )
                .with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::WindowHides)
                .with_menu(None),
        )
        .with_context(props)
        .launch(Root);
}

#[derive(Clone)]
struct RootProps {
    initial: Config,
    handles: Handles,
    events: Arc<Mutex<Option<Receiver<Event>>>>,
}

#[component]
fn Root() -> Element {
    let props = use_context::<RootProps>();

    let state = AppState {
        config: use_signal(|| props.initial.clone()),
        log: use_signal(Vec::new),
        present: use_signal(|| false),
        cooldown: use_signal(|| None),
        tool_ok: use_signal(|| true),
        devices_rev: use_signal(|| 0),
        monitors: use_signal(|| None),
    };
    use_context_provider(|| state);
    use_context_provider(|| props.handles.clone());
    use_hook(|| scan_monitors(state.monitors, tool_path(&props.initial, &props.handles.dir)));

    let tab = use_signal(|| Tab::Dashboard);

    // Drain the watcher channel into signals. The receiver is blocking, so it
    // lives on its own thread and hands values back through a Dioxus coroutine.
    let _pump = use_coroutine({
        let events = Arc::clone(&props.events);
        move |_rx: UnboundedReceiver<()>| {
            let mut state = state;
            let events = Arc::clone(&events);
            async move {
                let Some(rx) = events.lock().ok().and_then(|mut g| g.take()) else {
                    return;
                };
                let (async_tx, mut async_rx) = futures_channel::mpsc::unbounded();
                std::thread::spawn(move || {
                    for ev in rx {
                        if async_tx.unbounded_send(ev).is_err() {
                            return;
                        }
                    }
                });

                use futures_util::StreamExt;
                while let Some(ev) = async_rx.next().await {
                    match ev {
                        Event::PresenceChanged(p) => state.present.set(p),
                        Event::Cooldown(c) => state.cooldown.set(c),
                        Event::DevicesChanged => state.devices_rev += 1,
                        Event::Log(entry) => {
                            state.log.with_mut(|l| {
                                l.push(entry);
                                if l.len() > MAX_LOG {
                                    let overflow = l.len() - MAX_LOG;
                                    l.drain(0..overflow);
                                }
                            });
                        }
                    }
                }
            }
        }
    });

    // Tray icon + menu. Built through dioxus-desktop's own wrapper
    // (`dioxus::desktop::trayicon`) rather than the raw `tray-icon` crate
    // directly, so its internal tray-icon version can't drift from ours and
    // the icon is always constructed by Dioxus on the same thread that ends
    // up owning the platform event loop (the trap the brief calls out).
    // `use_hook` runs its closure exactly once per component instance, which
    // is what guarantees `init_tray_icon` is only ever called once.
    #[cfg(windows)]
    {
        use dioxus::desktop::trayicon::{
            Icon, init_tray_icon,
            menu::{Menu, MenuItem},
        };

        let (open_id, quit_id) = use_hook(|| {
            let menu = Menu::new();
            let open = MenuItem::new("Open", true, None);
            let quit = MenuItem::new("Quit", true, None);
            let open_id = open.id().clone();
            let quit_id = quit.id().clone();
            let _ = menu.append(&open);
            let _ = menu.append(&quit);

            let icon = Icon::from_path("icon.ico", Some((32, 32))).ok();
            init_tray_icon(menu, icon);

            (open_id, quit_id)
        });

        dioxus::desktop::use_tray_menu_event_handler(move |event| {
            let id = event.id();
            if *id == quit_id {
                std::process::exit(0);
            } else if *id == open_id {
                dioxus::desktop::window().set_visible(true);
            }
        });
    }

    rsx! {
        document::Style { {include_str!("../../assets/style.css")} }
        div { class: "shell",
            nav { class: "sidebar",
                NavButton { tab: Tab::Dashboard, current: tab, label: "Dashboard" }
                NavButton { tab: Tab::Devices,   current: tab, label: "Devices" }
                NavButton { tab: Tab::Monitors,  current: tab, label: "Monitors" }
                NavButton { tab: Tab::Settings,  current: tab, label: "Settings" }
                {debug_nav_button(tab)}
            }
            main { class: "content",
                match tab() {
                    Tab::Dashboard => rsx! { dashboard::Dashboard {} },
                    Tab::Devices   => rsx! { devices::Devices {} },
                    Tab::Monitors  => rsx! { monitors::Monitors {} },
                    Tab::Settings  => rsx! { settings::Settings {} },
                    #[cfg(not(windows))]
                    Tab::Debug     => rsx! { debug::Debug {} },
                }
            }
        }
    }
}

// `#[cfg]` on an rsx! child node isn't valid rsx syntax (the macro has its
// own grammar, not general Rust attributes), so the Debug tab's real
// presence/absence of `Tab::Debug` is gated here, outside the macro, instead.
#[cfg(not(windows))]
fn debug_nav_button(current: Signal<Tab>) -> Element {
    rsx! { NavButton { tab: Tab::Debug, current, label: "Debug" } }
}

#[cfg(windows)]
fn debug_nav_button(_current: Signal<Tab>) -> Element {
    rsx! {}
}

#[component]
fn NavButton(tab: Tab, current: Signal<Tab>, label: String) -> Element {
    let active = current() == tab;
    rsx! {
        button {
            class: if active { "active" } else { "" },
            onclick: move |_| current.set(tab),
            "{label}"
        }
    }
}
