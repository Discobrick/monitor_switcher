# Monitor Switcher

Monitor Switcher is a utility that monitors the presence of specific USB (HID) devices and executes commands to switch monitor inputs accordingly. It is particularly useful for switching monitor inputs automatically when a USB switch or KVM is toggled.

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
    Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -match 'VID_[0-9A-F]{4}&PID_[0-9A-F]{4}' } | Select-Object FriendlyName, InstanceId
    ```
3.  Look for your device in the `FriendlyName` column.
4.  The `InstanceId` column will contain the `VID_XXXX&PID_YYYY` string you need.

### Option 2: Using Device Manager

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

## Commands

The `connect_cmds` and `disconnect_cmds` sections contain the arguments that will be passed to `ControlMyMonitor.exe`.

*   `connect_cmds`: Executed when any of the monitored devices are detected (e.g., switching to local input).
*   `disconnect_cmds`: Executed when ALL monitored devices are removed (e.g., switching to remote input).
