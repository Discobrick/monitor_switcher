// Module root. main.rs is a thin binary entry point; real modules (hardware,
// watcher, ui, ...) get added here as later tasks land.
pub mod app;
pub mod config;
pub mod hardware;
pub mod watcher;
