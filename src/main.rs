// Release builds are a tray app with no console window; debug builds keep the
// console so logs and panics stay visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use monitor_switcher::{app, ui, watcher};

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = app::app_dir()?;
    std::env::set_current_dir(&dir)?;

    let cfg = monitor_switcher::config::Config::load(&dir)?;
    let shared = Arc::new(Mutex::new(cfg.clone()));

    let (event_tx, event_rx) = channel();
    let (cmd_tx, _wake_tx) = watcher::spawn(Arc::clone(&shared), dir.clone(), event_tx);

    let handles = ui::Handles {
        dir,
        shared_config: shared,
        commands: cmd_tx,
        #[cfg(not(windows))]
        wake: _wake_tx,
    };

    ui::launch(cfg, handles, event_rx);
    Ok(())
}
