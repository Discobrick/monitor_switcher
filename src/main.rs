use monitor_switcher::{app, watcher};

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = app::app_dir()?;
    std::env::set_current_dir(&dir)?;

    // `load_config` (not the bare `Config::load`) is the real entry point: it
    // upgrades a pre-versioning config on the way in, which is the point of
    // Task 8 wiring `migrate_v1` into the load path at all.
    let cfg = Arc::new(Mutex::new(app::load_config(&dir)?));

    let (event_tx, event_rx) = channel();
    let cmd_tx = watcher::spawn(Arc::clone(&cfg), dir.clone(), event_tx);

    // Temporary headless console output; replaced by the UI in Task 9.
    for event in event_rx {
        println!("{event:?}");
    }

    drop(cmd_tx);
    Ok(())
}
