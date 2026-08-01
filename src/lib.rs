// ponytail: exists only so `cargo test --lib` can exercise `config` without
// linking the tray-icon binary (which needs system libxdo, absent on this
// Linux dev box). main.rs keeps its own `mod config;` for the real binary.
pub mod config;
