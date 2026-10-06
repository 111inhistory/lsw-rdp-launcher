# RDP Launcher & RemoteApp Lifecycle Suite

An end-to-end RemoteApp integration and automated VM lifecycle management workflow designed for modern Linux Wayland environments (such as [Niri](https://github.com/YaLTeR/niri)) and Windows KVM guest virtual machines.

---

## 🌟 Key Features

- **Native Wayland RemoteApp**: Launches individual Windows applications seamlessly as native Wayland windows via FreeRDP SDL3 RAIL with hardware scaling and client-side clipboard integration.
- **Secure Keyring Auto-Login**: Passwords are saved in the Linux Secret Service (GNOME Keyring / KeepassXC) and piped directly into FreeRDP's memory via an anonymous stdin pipe (`/from-stdin:force`). Zero plaintext passwords in config files, shell history, or `/proc`.
- **Application Discovery & 256x256 Icons**: Scans Start Menu, Desktop shortcuts, and UWP/MSIX modern apps (`shell:AppsFolder`). Automatically deduplicates Desktop shortcuts against Start Menu targets, extracts native 256x256 PNG icons, and creates local `.desktop` desktop entries for app launchers (e.g. `dms`, `fuzzel`, `rofi`).
- **Headless SSH Guard**: Detects if commands are executed inside a headless SSH shell without an active `$WAYLAND_DISPLAY` or display socket, providing clear actionable error messages instead of failing silently.
- **Smart VM Lifecycle Management**:
  - **30s Idle Disconnect**: Automatically disconnects FreeRDP after 30 seconds of no active RemoteApp windows on the compositor.
  - **virtio-mem Dynamic Reclaim**: Reclaims dynamic `virtio-mem` RAM to 0 before VM suspension, shrinking guest memory footprint to the baseline.
  - **5m Inactivity Suspend**: Suspends the VM to RAM (0% CPU) after 5 minutes of total inactivity.
  - **SSH Protection**: Monitors socket connections (`ss -Htn`) across all dynamic VM IP addresses. If active SSH sessions are connected to the guest, VM suspension is safely inhibited.
  - **Instant Wake on Click**: Launching any RemoteApp shortcut while the VM is suspended automatically resumes the VM and connects in ~6 seconds.
  - **Libvirt Hook Hooked**: The lifecycle service does **not** autostart on system boot; it starts and stops strictly in sync with the VM's lifecycle via Libvirt QEMU hooks.
- **Interactive CLI Configuration**: Modify, inspect, and list all settings directly via `rdp-launcher config set/get/list` without ever needing to manually edit TOML files.

---

## 🏗️ Architecture

```
Linux Host (Wayland / Niri)                             Windows 11 KVM Guest
┌──────────────────────────────┐                       ┌──────────────────────────────┐
│  Desktop Launchers / CLI     │                       │  remoteapp-launcher.exe      │
│  (rdp-launcher run <app>)    │                       │  (Tray Daemon, Mutex Singleton)
└──────────────┬───────────────┘                       └──────────────▲───────────────┘
               │                                                      │
               ├───────> Secret Service (Keyring)                     │
               │                                                      │
               ├───────> Libvirt Hook / virsh ────────────────────────┤
               │         (resume, suspend, virtio-mem)                │
               │                                                      │
               ├───────> TCP JSON-RPC (Port 49152) ───────────────────┘
               │         (run / open / list_apps / drives / status)   │
               │                                                      │
┌──────────────▼───────────────┐                       ┌──────────────▼───────────────┐
│  FreeRDP (SDL3) Client       │◄═══════ RDP / RAIL ═══╡  Windows Desktop / DWM       │
│  (sdl-freerdp /app:program)  │        (Port 3389)    │  (Individual App Windows)    │
└──────────────────────────────┘                       └──────────────────────────────┘
```

---

## 📁 Repository Structure

```text
.
├── Cargo.toml               # Linux host launcher package manifest
├── src/                     # Linux host launcher source code
│   ├── client.rs            # TCP JSON-RPC agent client & request dispatcher
│   ├── config.rs            # TOML config management & CLI getter/setters
│   ├── lifecycle.rs         # Background watcher (SSH guard, idle & virtio-mem)
│   ├── mounts.rs            # Samba sync & bidirectional path translation
│   └── main.rs              # CLI entry point, FreeRDP spawner & app launcher
├── windows-agent/           # Windows guest daemon source code
│   ├── Cargo.toml           # Windows guest agent manifest
│   ├── install.ps1          # Windows guest installation & registry setup script
│   └── src/main.rs          # Daemon, TCP JSON-RPC server, 256x256 icon extractor
├── docs/                    # Architectural & operational documentation
│   ├── architecture.md      # Detailed system architecture & sequence flows
│   ├── configuration.md     # Configuration keys & CLI options reference
│   ├── mounts-and-paths.md  # Mount discovery & bidirectional path translation
│   └── vm-lifecycle.md      # Inactivity rules, virtio-mem reclaim, & SSH guard
├── scripts/                 # Systemd, SELinux, and host deployment scripts
│   ├── install-host.sh      # Linux host installation script
│   ├── uninstall-host.sh    # Linux host uninstallation script
│   ├── rdp-lifecycle.service# Systemd service unit template
│   ├── virtqemud_custom.te  # SELinux policy source for Libvirt hook & audit
│   └── virtqemud_custom.pp  # Compiled SELinux policy module
├── .github/workflows/       # GitHub Actions CI/CD workflows
│   ├── ci.yml               # Automated multi-platform build testing
│   └── release.yml          # Automated release packaging on git tags
├── test_full_lifecycle.py   # Automated end-to-end integration test suite
└── README.md                # Documentation
```

---

## 🚀 Quick Start & Installation

### 1. Build via GitHub Actions (Recommended)
This repository includes automated GitHub Actions workflows:
- Pushing to `master` builds both Linux and Windows binaries as artifacts.
- Pushing a tag (e.g. `v0.1.0`) automatically packages `rdp-launcher-linux-x86_64.tar.gz` and `remoteapp-launcher-windows-x86_64.zip` into a GitHub Release.

### 2. Windows Guest Installation
Run PowerShell as Administrator inside the Windows VM:
```powershell
# In windows-agent directory:
.\install.ps1
```
This script:
1. Builds `remoteapp-launcher.exe` in release mode.
2. Copies it to `%LOCALAPPDATA%\Programs\RemoteAppLauncher\remoteapp-launcher.exe`.
3. Adds the directory to the user's `PATH`.
4. Registers the launcher in the Terminal Server `TSAppAllowList` registry.

### 3. Linux Host Installation
Run the installer on the Linux host:
```bash
./scripts/install-host.sh
```
This script:
1. Builds and installs `rdp-launcher` to `/usr/local/bin/rdp-launcher`.
2. Restores appropriate SELinux file labels (`bin_t`).
3. Installs the SELinux policy module (`virtqemud_custom.pp`) if on Fedora/RHEL.
4. Registers `/etc/systemd/system/rdp-lifecycle.service` (kept disabled on boot).

---

## ⚙️ Configuration Management

All settings are stored in `~/.config/rdp-launcher/config.toml`. **You never need to edit the file by hand.** Use the interactive CLI commands:

### List All Configuration Keys
```bash
rdp-launcher config
```

### View or Modify Settings
```bash
# Set Windows VM IP or hostname
rdp-launcher config set server.host 192.168.122.14

# Set idle disconnect timeout (in seconds, 0 = disabled)
rdp-launcher config set lifecycle.idle_disconnect_timeout 30

# Set VM suspend timeout (in seconds, 0 = disabled)
rdp-launcher config set lifecycle.vm_suspend_timeout 300

# Set custom FreeRDP scale factor
rdp-launcher config set freerdp.scale 140

# Get a specific value
rdp-launcher config get server.host
```

### Configure Keyring Password
```bash
# Save Windows account password securely in Keyring
rdp-launcher set-password

# Check password status
rdp-launcher status
```

---

## 💻 CLI Usage Guide

```text
Usage: rdp-launcher [OPTIONS] [COMMAND]

Commands:
  set-password      Save or update the Windows password in the system Keyring
  status            Check if a password exists in Keyring
  clear-password    Delete the saved password from Keyring
  config            View, get, or set configuration values directly from the CLI
  sync-apps         Fetch and sync applications from the remote Windows host
  list-apps         List previously synced applications
  run               Run an application on the remote host (auto-wakes VM)
  stop-daemon       Stop the remote daemon session and FreeRDP
  watcher           Run the lifecycle watcher service
  lifecycle-status  Show lifecycle status of VM, SSH, FreeRDP and RemoteApp windows
  launch            Launch FreeRDP with the daemon or specific application
```

### Synchronizing Remote Apps
To scan the Windows guest and generate `.desktop` entries in `~/.local/share/applications/`:
```bash
rdp-launcher sync-apps
```
All discovered applications (with high-resolution 256x256 PNG icons) will immediately appear in your desktop application launcher (e.g. Niri, DMS, Rofi).

### Launching an Application
```bash
# Launch by shortcut name, app ID, or Windows path
rdp-launcher run notepad
rdp-launcher run character-map
rdp-launcher run "C:\Program Files\Microsoft Office\root\Office16\WINWORD.EXE"
```

### Inspecting Lifecycle Status
```bash
rdp-launcher lifecycle-status
```
Example Output:
```text
=== RDP & VM Lifecycle Status ===
VM Name:             win11
VM State:            运行
VM IP Addresses:     192.168.122.14
Active SSH Sessions: None
FreeRDP Connection:  Connected
RemoteApp Windows:   1 active
  - 记事本

Configured Timeouts:
  - Idle RDP Disconnect: 30s
  - Idle VM Suspend:    300s
  - Reclaim virtio-mem: true (alias: ua-virtiomem0)
```

---

## 📂 Cross-OS Mounts & Document Opening

Seamlessly bridge host files with Windows guest applications (like Microsoft 365 Word/Excel) without copying files.

### 1. View & Synchronize Mount Table
Automatically correlates Linux Samba shares (`/etc/samba/smb-win11.conf`) with Windows mapped network drives (`Win32_LogicalDisk`):
```bash
rdp-launcher mounts sync    # Discover and save mount mappings
rdp-launcher mounts         # View active mount table
```

### 2. Bidirectional Path Translation (`path`)
Translate paths between Linux and Windows:
```bash
# Linux to Windows
rdp-launcher path /data/daily_data/finance/q3.xlsx
# Output: J:\finance\q3.xlsx

# Windows to Linux
rdp-launcher path 'J:\finance\q3.xlsx'
# Output: /data/daily_data/finance/q3.xlsx
```

### 3. Open Linux Files in Windows RemoteApp (`open`)
Open any Linux document directly in its associated Windows software:
```bash
# Opens directly in Microsoft 365 Excel
rdp-launcher open /data/daily_data/finance/q3.xlsx

# Or explicitly choose an application
rdp-launcher open /home/skwj111/contract.docx --app word
```

---

## 🔗 Libvirt Hook Integration

To hook `rdp-lifecycle` strictly to your VM's runtime, add the following to `/etc/libvirt/hooks/qemu`:

```bash
#!/usr/bin/env bash
# /etc/libvirt/hooks/qemu

GUEST_NAME="${1:-}"
ACTION="${2:-}"
PHASE="${3:-}"

if [ "$GUEST_NAME" = "win11" ]; then
    case "$ACTION" in
        started)
            if [ "$PHASE" = "begin" ]; then
                /usr/bin/systemctl start rdp-lifecycle.service || true
            fi
            ;;
        stopped|release)
            if [ "$PHASE" = "end" ]; then
                /usr/bin/systemctl stop rdp-lifecycle.service || true
            fi
            ;;
    esac
fi
```

---

## 🧪 Automated Testing

An automated end-to-end integration test suite is provided in `test_full_lifecycle.py`:

```bash
python3 test_full_lifecycle.py
```
This tests:
1. Cold VM shutdown verification.
2. Libvirt Hook automatic service dispatch.
3. Windows cold boot & RDP port readiness.
4. Keyring credential auto-login & FreeRDP session setup.
5. RemoteApp mapping into Niri compositor.
6. 30s idle disconnect countdown.
7. `virtio-mem` memory dynamic deflation to 0.
8. VM suspend to RAM & instant wake recovery on app click.

---

## 📜 License

Licensed under the [MIT License](LICENSE).
