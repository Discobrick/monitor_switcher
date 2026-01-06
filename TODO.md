# Monitor Switcher - Dioxus Desktop App Design Document

This document outlines the plan for creating a desktop GUI for the `monitor_switcher` utility using the Dioxus framework.

## 1. Project Overview
- **Framework**: Dioxus (Desktop)
- **Goal**: A standalone application that combines the monitoring logic with a modern, dark-mode configuration dashboard.
- **Theme**: Minimalist, Dark Mode (IntelliJ/VSCode inspired palette).

## 2. Core Features

### 2.1 Dependency Management
- **Automatic Check**: On startup, verify if `ControlMyMonitor.exe` exists in the application directory or system PATH.
- **Download Assistant**: 
    - If missing, show a prominent "Download Dependency" button.
    - Automated download and extraction of `controlmymonitor.zip` from NirSoft's official site.
    - Manual "Browse" button to locate the executable if already downloaded elsewhere.

### 2.2 Dashboard & Live Monitoring
- **Monitoring Toggle**: Global switch to start/stop the background USB monitoring.
- **Live Event Log**: 
    - Real-time scrolling log showing `[CONNECTED]` and `[DISCONNECTED]` events.
    - Display timestamps, device names (FriendlyName), and Hardware IDs (VID_XXXX&PID_YYYY).
    - Purpose: Helps users identify which device is which without needing external tools.

### 2.3 Configuration Management
- **USB Device Picker**:
    - List of currently monitored devices with "Remove" buttons.
    - "Add Device" button that opens a list of all currently connected HID devices.
    - Clicking a connected device automatically adds its VID/PID to the config.
- **Command Builder**:
    - Manage `connect_cmds` and `disconnect_cmds`.
    - **Monitor Selector**: Dropdown showing available monitors (parsed from `ControlMyMonitor.exe /scomma`).
    - **VCP Helper**: Preset for "Input Select" (VCP 60) with common values (DP, HDMI, etc.), while allowing custom VCP codes.
    - **Command Preview**: Show the actual command string being built.

### 2.4 System Integration
- **Tray Icon**: 
    - Minimize to tray.
    - Right-click menu for quick actions (Toggle Monitoring, Settings, Exit).
    - Notifications on device change (optional toggle).
- **Auto-Start**:
    - "Run at Startup" setting that creates/removes a shortcut in `shell:startup`.

## 3. Technical Implementation Plan

### Phase 1: Setup & Scaffolding
- [ ] Initialize Dioxus Desktop project.
- [ ] Integrate `tokio` for async tasks.
- [ ] Migrate `Config` struct and TOML handling from current `main.rs`.
- [ ] Set up basic Dark Mode theme and layout (Sidebar/Tabs).

### Phase 2: Dependency & Service Logic
- [ ] Implement `ControlMyMonitor.exe` presence check.
- [ ] Implement automated download/unzip utility.
- [ ] Port the background monitoring loop into a Dioxus `spawn` task or a separate thread.
- [ ] Implement communication between background thread and UI (using channels/state).

### Phase 3: UI Development
- [ ] **Dashboard**: Status indicator and Live Log.
- [ ] **Device Settings**: Connected devices list and "Add" dialog.
- [ ] **Command Settings**: Command list and Builder UI.
- [ ] **General Settings**: Auto-start, path configuration, theme toggle.

### Phase 4: Refinement
- [ ] Add tray icon support using `tray-icon` crate (integrated with Dioxus window).
- [ ] Implement "Run at Startup" logic.
- [ ] Polish UI with transitions and consistent spacing.

## 4. Design Questions & Considerations
- **Legal**: Automated download of NirSoft tools is generally accepted for personal use, but the app should clearly attribute the tool to NirSoft and include a link to their website.
- **Persistence**: Ensure `config.toml` is saved whenever a change is made in the UI.
- **Error Handling**: Graceful handling if `ControlMyMonitor.exe` fails to execute or returns an error.
