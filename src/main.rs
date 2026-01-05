// #![windows_subsystem = "windows"] // Commented out so you can see the console

use std::process::Command;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use serde::Deserialize;

use windows::core::{PCWSTR, w};
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Gdi::HBRUSH;
use tray_icon::{TrayIconBuilder, menu::{Menu, MenuItem}, TrayIconEvent};

#[derive(Deserialize)]
struct Config {
    monitored_devices: Vec<String>,
    disconnect_cmds: Vec<String>,
    connect_cmds: Vec<String>,
}

// AtomicBool is thread-safe and doesn't require 'unsafe' to read or write
static WAS_CONNECTED: AtomicBool = AtomicBool::new(true);
const MONITOR_TOOL: &str = "ControlMyMonitor.exe";

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("--- Initializing USB Monitor ---");
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path.parent().ok_or("Could not find executable directory")?;
    std::env::set_current_dir(exe_dir)?;

    println!("Current Directory set to: {:?}", exe_dir);

    // 1. Load Config
    let config_str = fs::read_to_string("config.toml")?;
    let config: Config = toml::from_str(&config_str)?;
    println!("Loaded config with {} monitored devices.", config.monitored_devices.len());

    // 2. Setup Tray Menu
    let tray_menu = Menu::new();
    let quit_item = MenuItem::new("Quit", true, None);
    let _ = tray_menu.append(&quit_item).map_err(|e| e.to_string())?;

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Monitor USB Switcher")
        .build()
        .map_err(|e| e.to_string())?;

    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .map_err(|e| e.to_string())?;

        let class_name = w!("USB_MONITOR_CLASS");

        let wnd_class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: class_name,
            lpfnWndProc: Some(wnd_proc),
            hbrBackground: HBRUSH(0),
            ..Default::default()
        };

        RegisterClassW(&wnd_class);

        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            PCWSTR::null(),
            WS_OVERLAPPED,
            0, 0, 0, 0,
            None, None, instance, None,
        );
        println!("Background window created. Listening for device changes...");
    }

    // 3. Main Message Loop
    let mut message = MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            TranslateMessage(&message);
            DispatchMessageW(&message);

            if let Ok(_event) = TrayIconEvent::receiver().try_recv() {
                // Exit logic could go here
            }
        }
    }

    
    Ok(())
}

static LAST_TRIGGER: Mutex<Option<Instant>> = Mutex::new(None);
const COOLDOWN_DURATION: Duration = Duration::from_secs(5);

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DEVICECHANGE => {
            if wparam.0 == 0x0007 { // DBT_DEVNODES_CHANGED
                let mut last_trigger = LAST_TRIGGER.lock().unwrap();

                // Check if we are still in the cooldown period
                if let Some(instant) = *last_trigger {
                    if instant.elapsed() < COOLDOWN_DURATION {
                        // Ignore this event, it's likely just the monitors refreshing
                        return LRESULT(0);
                    }
                }

                println!("\n[Event] Hardware change detected. Checking state...");
                check_usb_state();

                // Update the last trigger time
                *last_trigger = Some(Instant::now());
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn check_usb_state() {
    let Ok(config_str) = fs::read_to_string("config.toml") else {
        println!("[Error] Could not read config.toml");
        return
    };
    let Ok(config) = toml::from_str::<Config>(&config_str) else {
        println!("[Error] Could not parse config.toml");
        return
    };

    let mut any_found = false;

    // 1. Hardware Detection
    for id in &config.monitored_devices {
        // We removed "-Status OK" and replaced it with a filter for 'Present'
        // This is more reliable for devices that just woke up
        let script = format!(
            "Get-PnpDevice -InstanceId *{}* | Where-Object {{ $_.Present -eq $true }}",
            id
        );

        let output = Command::new("powershell")
            .args(&["-Command", &script])
            .output();

        if let Ok(out) = output {
            if !out.stdout.is_empty() {
                any_found = true;
                // Add a debug print here to see WHICH device was found
                println!("[Debug] Found active device: {}", id);
                break;
            }
        }
    }

    // 2. Thread-Safe State Management
    // Ordering::SeqCst ensures all threads see the same value at the same time
    let previously_connected = WAS_CONNECTED.load(Ordering::SeqCst);

    if !any_found && previously_connected {
        // Transition: Connected -> Disconnected
        println!(">>> USB DEVICES DISCONNECTED: Switching to Remote Input...");
        execute_commands(&config.disconnect_cmds);
        WAS_CONNECTED.store(false, Ordering::SeqCst);

    } else if any_found && !previously_connected {
        // Transition: Disconnected -> Connected
        println!(">>> USB DEVICES RECONNECTED: Switching to Local Input...");
        execute_commands(&config.connect_cmds);
        WAS_CONNECTED.store(true, Ordering::SeqCst);
    }
    // Note: If state hasn't changed, we do absolutely nothing (prevents loops)
}

// Helper function to run commands and capture errors
fn execute_commands(args_list: &[String]) {
    // Ensure we are looking in the same directory as the app
    let cur_dir = std::env::current_dir().unwrap_or_default();

    for args_str in args_list {
        println!("[System] Tool: {} | Args: {}", MONITOR_TOOL, args_str);

        // Use a smarter split to handle the quoted monitor paths correctly
        let args = parse_args(args_str);

        let process = Command::new(MONITOR_TOOL)
            .args(&args)
            .current_dir(&cur_dir)
            .output();

        match process {
            Ok(output) => {
                if !output.status.success() {
                    let err = String::from_utf8_lossy(&output.stderr);
                    println!("[Tool Error] {}", err.trim());
                } else {
                    println!("[Success] Command sent to {}", MONITOR_TOOL);
                }
            }
            Err(e) => println!("[System Error] Could not find {}: {}", MONITOR_TOOL, e),
        }
    }
}

/// Helper to split arguments while respecting quotes (e.g. "\\.\Display1")
fn parse_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for c in input.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ' ' if !in_quotes => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}