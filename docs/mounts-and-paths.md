# Mounts, Paths, and File Associations

This document explains how host directories, Samba shares, Windows network drives, and FreeDesktop MIME associations are linked.

---

## 1. Mount Table Discovery & Synchronization

The mount table bridges Linux directories with Windows drive letters.

```
Linux Host Directory           Samba Share               Windows Guest
/data/games            ───►   [games]            ───►    F:\
/data/daily_data       ───►   [daily_data]       ───►    J:\
/data/big_file         ───►   [big_file]         ───►    M:\
/home/skwj111          ───►   [home]             ───►    Y:\
```

### Discovery Logic
Running `rdp-launcher mounts sync`:
1. Reads `/etc/samba/smb-win11.conf` (world-readable, permissions `0644`).
2. Extracts share sections (`[games]`, `[daily_data]`, etc.) and their `path = ...` directives.
3. Queries Windows guest network drives directly via TCP using native Win32 API (`WNetGetConnectionW` from `mpr.dll`):
   - Zero SSH overhead, zero PowerShell, sub-millisecond query time.
4. Correlates UNC paths (e.g. `\\192.168.122.1\games`) with Samba shares to produce the mapping table:
   - `F:\` <=> `/data/games`
   - `J:\` <=> `/data/daily_data`
   - `M:\` <=> `/data/big_file`
   - `Y:\` <=> `/home/skwj111`
5. Stores the mappings into `config.toml` under `[mounts]`.

---

## 2. Bidirectional Path Translation (`path`)

The `path` command uses canonical longest-prefix matching to translate file and directory paths between Linux and Windows:

```bash
# Linux to Windows
rdp-launcher path /data/daily_data/reports/summary.xlsx
# -> J:\reports\summary.xlsx

# Windows to Linux
rdp-launcher path 'J:\reports\summary.xlsx'
# -> /data/daily_data/reports/summary.xlsx
```

If a Linux path is outside all configured mounts, `rdp-launcher` refuses to guess and prints a clear error listing all valid mounts.

---

## 3. Transparent File Opening (`open`)

To open a host file in a Windows application (such as Microsoft 365 Excel):

```bash
rdp-launcher open /data/daily_data/reports/summary.xlsx
```

### Workflow
1. Decodes and sanitizes the input using the standard `url` RFC 3986 crate (handling `file://` URIs and URL-encoded percent sequences from file managers).
2. Maps the Linux path to its Windows equivalent (`J:\reports\summary.xlsx`).
3. Resumes the VM and establishes FreeRDP session if not already active.
4. Dispatches the command to `remoteapp-launcher.exe run "J:\reports\summary.xlsx"` over SSH.
5. In Windows Session 2, the daemon's FIFO queue dispatches `ShellExecuteExW` with verb `"open"`.
6. Windows resolves the file extension to its default application (e.g. Microsoft 365 Excel) and renders the window into Niri via RAIL.

---

## 4. FreeDesktop MIME & Desktop Integration

When running `rdp-launcher sync-apps`:
- Scans applications from Windows guest.
- Applies user-defined rules from `[[overrides]]` in `config.toml`.
- For matched applications (e.g. `excel`, `word`, `powerpoint`), writes `.desktop` entries with:
  ```ini
  Exec=rdp-launcher open --app excel %U
  MimeType=application/vnd.openxmlformats-officedocument.spreadsheetml.sheet;application/vnd.ms-excel;text/csv;
  ```
- File managers (Nautilus, Dolphin, Thunar) can then directly invoke `rdp-launcher open` when double-clicking or right-clicking documents.
