# VM & RDP Lifecycle Management

This document details the automated power and memory lifecycle management between the Linux host and Windows guest VM.

---

## 1. Zero-Boot Background Policy

By design, `rdp-lifecycle.service` **does not autostart on host boot** (`systemctl is-enabled` is `disabled`). It is lifecycle-linked to the VM:
- **VM Start**: `/etc/libvirt/hooks/qemu` fires the `started begin` hook, which runs:
  ```bash
  systemctl start rdp-lifecycle.service
  ```
- **VM Shutdown / Release**: `/etc/libvirt/hooks/qemu` fires `stopped end` or `release end`:
  ```bash
  systemctl stop rdp-lifecycle.service
  ```
This ensures zero background overhead on the host when the VM is not running.

---

## 2. Inactivity Rules & Timers

```
[ Application Open in Niri ]
             │
             ▼ (User closes last RemoteApp window)
[ 30s Idle Disconnect Timer Starts ]
             │
             ├─── If new window opens ───► Timer Reset
             ▼ (30 seconds elapsed)
[ Disconnect FreeRDP Session ] (VM still running in background)
             │
             ▼
[ 300s (5-minute) VM Inactivity Timer Starts ]
             │
             ├─── If SSH connection active (ss -Htn) ───► Inhibits suspend indefinitely
             │
             ▼ (300 seconds elapsed without RDP or SSH)
[ 1. Reclaim virtio-mem dynamic memory to 0 ]
  `virsh update-memory-device win11 --alias ua-virtiomem0 --requested-size 0 --live`
             │
             ▼
[ 2. Suspend VM to RAM (0% CPU) ]
  `virsh suspend win11`
```

---

## 3. SSH Protection Mechanism

If you are connected to the Windows guest via SSH (for remote terminal work, compilation, or diagnostics), the VM must **never** suspend unexpectedly.

The lifecycle watcher employs a dual-tier protection mechanism:
1. **Primary (Windows Kernel Win32 API)**: Queries `remoteapp-launcher.exe` via TCP JSON-RPC (`status` action). The Windows agent executes native Win32 `GetExtendedTcpTable` (from `iphlpapi.dll`) to count all active IPv4 and IPv6 connections on port 22 in `MIB_TCP_STATE_ESTAB` directly inside the Windows kernel.
2. **Fallback (Host `ss` Probe)**: If the agent is unreachable or recovering, the host watcher resolves all dynamic IP addresses assigned to the VM (via QEMU Guest Agent `virsh domifaddr`) and inspects sockets on the host bridge:
```bash
ss -Htn 'dport = :22 and dst <vm_ip>'
```
- If any active SSH socket in state `ESTAB` is detected by either mechanism, the suspend timer is immediately reset.

---

## 4. virtio-mem Dynamic Reclaim Before Suspend

When Windows runs intensive tasks, `virtio-mem` expands guest RAM. Suspending an 8GB or 16GB VM consumes host memory.
Before issuing `virsh suspend`, `rdp-launcher` executes:
```bash
virsh update-memory-device <vm_name> --alias ua-virtiomem0 --requested-size 0 --live
```
This forces the Windows virtio-mem balloon driver to release unneeded dynamic memory blocks back to the Linux host, compressing the VM footprint down to the baseline (~3GB) while suspended in RAM.

---

## 5. Instant Wake on Click (~6 Seconds)

When you click an application shortcut or run `rdp-launcher open <file>`:
1. `rdp-launcher` queries `virsh domstate`.
2. If state is `paused` / `suspended`: calls `virsh resume <vm_name>`.
3. Waits for RDP port 3389 and connects FreeRDP.
4. Total latency from suspended state to application window appearing on Niri: **approx. 6 seconds**.
