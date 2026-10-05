# Architecture & Design

`lsw-rdp-launcher` bridges a Linux host (Wayland compositor such as Niri) with a Windows KVM guest, enabling native desktop integration of Windows applications via FreeRDP RAIL.

---

## 1. High-Level System Architecture

```
                 Linux Host (Wayland / Niri)
┌─────────────────────────────────────────────────────────────┐
│  Desktop Launcher / File Manager (Nautilus, Dolphin, DMS)   │
│  [ Double click .xlsx / .docx or run shortcut ]             │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│  rdp-launcher (Host Dispatcher)                             │
│  1. Check graphical session ($WAYLAND_DISPLAY / socket)      │
│  2. Translate Linux path to Windows drive (via mounts table) │
│  3. If VM is paused/suspended: wake via `virsh resume`       │
│  4. If FreeRDP is down: spawn FreeRDP daemon session         │
│  5. Send dispatch command via SSH to Windows guest           │
└──────────────────────────────┬──────────────────────────────┘
                               │
                  ┌────────────┴────────────┐
                  │ SSH Command             │ RDP / RAIL
                  │ (daemon_app run <path>) │ (Port 3389)
                  ▼                         ▼
┌─────────────────────────────────────────────────────────────┐
│  Windows Guest (Session 2 / Interactive User Session)       │
│                                                             │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  remoteapp-launcher.exe (Daemon in Session 2)         │  │
│  │  - FreeRDP RAIL entry point                           │  │
│  │  - System tray icon with keep-alive power states       │  │
│  │  - Named pipe server: \\.\pipe\remoteapp_launcher     │  │
│  │  - FIFO Worker Queue (processes commands in order)    │  │
│  └───────────────────────────▲───────────────────────────┘  │
│                              │ Named Pipe IPC               │
│  ┌───────────────────────────┴───────────────────────────┐  │
│  │  remoteapp-launcher.exe (CLI client from SSH)         │  │
│  └───────────────────────────────────────────────────────┘  │
│                              │                              │
│                              ▼                              │
│  ShellExecuteExW (Open verb on target / document)           │
│  ==> Native Windows application (Excel, Word, etc.)         │
│      maps window directly back to Niri via RAIL             │
└─────────────────────────────────────────────────────────────┘
```

---

## 2. Division of Responsibilities

### Linux Host (`rdp-launcher`)
The host launcher acts as the orchestrator, lifecycle supervisor, and desktop integration gateway:
- **VM Lifecycle Supervisor**: Integrates with `/etc/libvirt/hooks/qemu` so the watcher only runs while the VM runs. Detects idle states (30s no windows -> RDP disconnect; 5m inactivity -> `virtio-mem` reclaim & VM suspend to RAM; SSH connection protection).
- **FreeRDP Session Manager**: Spawns FreeRDP **only once** to launch the remote daemon. FreeRDP never launches apps directly; it merely provides the graphical display channel.
- **Desktop & MIME Integration**: Scans remote applications, caches 256x256 PNG icons, generates `.desktop` files in `~/.local/share/applications/`, and registers MIME handlers.
- **Path Translation Engine**: Reads Samba configuration (`/etc/samba/smb-win11.conf`) and Windows mapped drive letters (`F:`, `J:`, `M:`, `Y:`), performing clean, bidirectional path translation.
- **Security**: Injects credentials from Linux Secret Service (Keyring) into FreeRDP via stdin (`/from-stdin:force`). Zero plaintext passwords on disk or command lines.

### Windows Guest (`remoteapp-launcher.exe`)
The guest launcher operates as a dual-role executable:
1. **Daemon Role (`remoteapp-launcher.exe daemon`)**:
   - Launched by FreeRDP as the primary `/app:program` RAIL application.
   - Sits in the system tray, preventing screen lock or sleep via `SetThreadExecutionState` and periodic keep-alive events.
   - Listens on named pipe `\\.\pipe\remoteapp_launcher`.
   - **FIFO Task Queue**: Manages an internal sequential channel (`mpsc`). When multiple open or run requests arrive, they are dispatched strictly in order to prevent Windows process contention.
   - Calls `ShellExecuteExW` within the interactive user session (Session 2), ensuring applications have full GUI desktop access.
2. **CLI Client Role (`remoteapp-launcher.exe run <target>`)**:
   - Invoked over SSH by the Linux host.
   - Connects to the daemon's local named pipe, delivers the launch request, and exits immediately.
3. **Application Scanner Role (`remoteapp-launcher.exe list-apps --with-icons`)**:
   - Scans Start Menu (`.lnk`), Desktop (`.lnk` deduplicated against Start Menu), and UWP/MSIX apps (`shell:AppsFolder`).
   - Extracts native 256x256 icons using `PrivateExtractIconsW` and encodes them as PNG Base64.
