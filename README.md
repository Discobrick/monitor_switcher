# Monitor Switcher

Monitor Switcher is a utility that monitors the presence of specific USB (HID) devices and executes commands to switch monitor inputs accordingly. It is particularly useful for switching monitor inputs automatically when a USB switch or KVM is toggled.

## Requirements

This utility requires `ControlMyMonitor.exe` from NirSoft to interact with your monitors.

1.  Download **ControlMyMonitor** from the official website: [https://www.nirsoft.net/utils/control_my_monitor.html](https://www.nirsoft.net/utils/control_my_monitor.html)
2.  Extract `ControlMyMonitor.exe` and place it in the same directory as the `monitor_switcher` executable.

## Configuration

The application is configured via `config.toml`.

### `monitored_devices`
This is a list of devices that the application will look for. The format must be `"VID_XXXX&PID_YYYY"`, where `XXXX` and `YYYY` are 4-digit hexadecimal values.

Example:
```toml
monitored_devices = [
    "VID_046D&PID_085C",
    "VID_3142&PID_0686"
]
```

## How to find VID and PID

To add a device to the `monitored_devices` list, you need to find its Vendor ID (VID) and Product ID (PID).

### Option 1: Using PowerShell (Quickest)

1.  Open **PowerShell**.
2.  Copy and paste the following command:
    ```powershell
    Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -match 'VID_([0-9A-F]{4})&PID_([0-9A-F]{4})' } | 
    Select-Object FriendlyName, @{n='HardwareID'; e={$Matches[0]}} | 
    Select-Object * -Unique | 
    Format-Table -AutoSize
    ```
3.  Look for your device in the `FriendlyName` column.
4.  The `InstanceId` column will contain the `VID_XXXX&PID_YYYY` string you need.

### Option 2: Live Monitor via PowerShell (Real-time)

To monitor USB/HID devices live, you can use WMI Event Watchers. Instead of running a command over and over, this script sits in the background and "listens" for the operating system to broadcast a device change.

This script will display the Device Name, the Hardware ID (VID/PID), and the Action (Connected or Disconnected) in real-time.

1.  Open **PowerShell as Administrator**.
2.  Copy and paste the following script and press **Enter**:
    ```powershell
    # 1. Define the queries to watch for Device Arrival and Removal
    $ArrivalQuery = "SELECT * FROM __InstanceCreationEvent WITHIN 2 WHERE TargetInstance ISA 'Win32_PnPEntity' AND TargetInstance.DeviceID LIKE 'USB%'"
    $RemovalQuery = "SELECT * FROM __InstanceDeletionEvent WITHIN 2 WHERE TargetInstance ISA 'Win32_PnPEntity' AND TargetInstance.DeviceID LIKE 'USB%'"

    # 2. Register the Events
    $ArrivalWatcher = Register-WmiEvent -Query $ArrivalQuery -Action {
        $Name = $EventArgs.NewEvent.TargetInstance.Name
        $Id = $EventArgs.NewEvent.TargetInstance.DeviceID
        if ($Id -match 'VID_([0-9A-F]{4})&PID_([0-9A-F]{4})') {
            $VidPid = $Matches[0]
            Write-Host "[CONNECTED]    $VidPid - $Name" -ForegroundColor Green
        }
    }

    $RemovalWatcher = Register-WmiEvent -Query $RemovalQuery -Action {
        $Name = $EventArgs.NewEvent.TargetInstance.Name
        $Id = $EventArgs.NewEvent.TargetInstance.DeviceID
        if ($Id -match 'VID_([0-9A-F]{4})&PID_([0-9A-F]{4})') {
            $VidPid = $Matches[0]
            Write-Host "[DISCONNECTED] $VidPid - $Name" -ForegroundColor Red
        }
    }

    Write-Host "Monitoring USB/HID changes... Press Ctrl+C to stop." -ForegroundColor Cyan

    # 3. Keep the script running
    try {
        do { Start-Sleep -Seconds 1 } while ($true)
    }
    finally {
        # 4. Clean up watchers when you stop the script
        Unregister-Event -SourceIdentifier $ArrivalWatcher.Name
        Unregister-Event -SourceIdentifier $RemovalWatcher.Name
        Write-Host "`nMonitoring stopped and events cleaned up." -ForegroundColor Yellow
    }
    ```
3.  Unplug/Plug in your device. You will see the status update instantly in the console.

### Option 3: Using Device Manager

1.  Connect the device you want to monitor.
2.  Right-click the **Start** button and select **Device Manager**.
3.  Locate your device in the list (usually under **Human Interface Devices**, **Mice and other pointing devices**, or **Keyboards**).
4.  Right-click the device and select **Properties**.
5.  Go to the **Details** tab.
6.  In the **Property** dropdown, select **Hardware Ids**.
7.  You will see one or more strings like `HID\VID_046D&PID_085C&REV_0100`.
    *   The `VID_` part followed by 4 hex characters is your Vendor ID (e.g., `046D`).
    *   The `PID_` part followed by 4 hex characters is your Product ID (e.g., `085C`).
8.  Combine them into the format `VID_XXXX&PID_YYYY` and add it to your `config.toml`.

**Note:** The application uses the `hidapi` library, so it can only detect devices that expose an HID (Human Interface Device) interface. Most keyboards, mice, and USB controllers do this.

## How to find Monitor Input values

The `connect_cmds` and `disconnect_cmds` use `ControlMyMonitor.exe` to change the monitor input. To configure these, you need the **Monitor Device Name** and the **Input Value**.

1.  Run `ControlMyMonitor.exe`. (If you don't have it, see [Requirements](#requirements))
2.  Select the monitor you want to configure from the dropdown at the top.
3.  The **Monitor Device Name** is displayed in the title bar or can be found by going to **File > Export Selected Monitors** (it looks like `\\.\DISPLAY1\Monitor0`).
4.  Look for the row with **VCP Code `60`** (Input Select).
5.  The **Current Value** column shows the value for the currently active input. (This can show up as a 4 digit number that is not one of the possible values, but that can be ignore, you need to test with the numbers available in the possible values column)
6.  To find the values for other inputs:
    *   Manually switch your monitor's input using its physical buttons.
    *   Press **F5** in `ControlMyMonitor.exe` to refresh.
    *   Note the new **Current Value** for VCP Code `60`.
7.  Common values for VCP Code `60`:
    *   `15`: DisplayPort 1
    *   `16`: DisplayPort 2
    *   `17`: HDMI 1
    *   `18`: HDMI 2

## Commands

The `connect_cmds` and `disconnect_cmds` sections contain the arguments that will be passed to `ControlMyMonitor.exe`.

*   `connect_cmds`: Executed when any of the monitored devices are detected (e.g., switching to local input).
*   `disconnect_cmds`: Executed when ALL monitored devices are removed (e.g., switching to remote input).

## Running at Startup (Windows)

To have Monitor Switcher start automatically when you log in to Windows:

1.  Press `Win + R` on your keyboard to open the **Run** dialog.
2.  Type `shell:startup` and press **Enter**. This opens the Startup folder.
3.  Right-click your `monitor_switcher.exe` and select **Create shortcut**.
4.  Move the newly created shortcut into the **Startup** folder you just opened.

**Note:** Ensure that `config.toml` and `ControlMyMonitor.exe` are in the same directory as `monitor_switcher.exe`.
