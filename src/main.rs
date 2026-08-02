use monitor_switcher::{app, ui, watcher};

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = app::app_dir()?;
    std::env::set_current_dir(&dir)?;

    // `load_config` (not the bare `Config::load`) upgrades a pre-versioning
    // config on the way in; see Task 8.
    let cfg = app::load_config(&dir)?;
    let shared = Arc::new(Mutex::new(cfg.clone()));

    let (event_tx, event_rx) = channel();
    let cmd_tx = watcher::spawn(Arc::clone(&shared), dir.clone(), event_tx);

    let handles = ui::Handles {
        dir,
        shared_config: shared,
        commands: cmd_tx,
        // ponytail: `watcher::spawn` owns and drops the non-Windows debug-wake
        // sender internally (Task 11's debug panel is what wires it up).
        #[cfg(not(windows))]
        wake: None,
    };

    ui::launch(cfg, handles, event_rx);
    Ok(())
}
