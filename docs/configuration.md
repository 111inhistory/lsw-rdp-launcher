# Configuration Reference

All settings are stored in `~/.config/rdp-launcher/config.toml`. Settings can be inspected and modified directly using the CLI.

---

## 1. CLI Configuration Management

```bash
# View all configuration keys, types, and current values
rdp-launcher config

# Retrieve a specific value
rdp-launcher config get server.host

# Set a configuration value
rdp-launcher config set server.host 192.168.122.14
rdp-launcher config set lifecycle.vm_suspend_timeout 600

# View raw pretty-printed TOML
rdp-launcher config show
```

---

## 2. Configuration Options

### `[server]`
| Key | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `host` | `string` | `"auto"` | Windows guest IP or hostname (`"auto"` queries guest agent) |
| `user` | `string` | `$USER` | Windows RDP user name |
| `service` | `string` | `"rdp-bridge"` | Keyring service name for Secret Service password lookup |
| `agent_port` | `u16` | `49152` | Windows Guest Agent TCP JSON-RPC listener port |

### `[freerdp]`
| Key | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `bin` | `string` | Auto-detect | Path or binary name for `sdl-freerdp` |
| `video_driver` | `option<string>`| `"wayland"` | SDL video backend (`"wayland"`, `"x11"`, or `none`) |
| `scale_desktop`| `option<u32>` | `200` | FreeRDP desktop scaling percentage |
| `scale` | `option<u32>` | `none` | FreeRDP UI scaling percentage |
| `clipboard` | `bool` | `true` | Enable clipboard synchronization |
| `cert_ignore` | `bool` | `true` | Ignore self-signed RDP TLS certificates |
| `log_filters` | `option<string>`| `DEBUG` | FreeRDP log verbosity filter |
| `extra_args` | `list<string>` | `[]` | Additional flags passed to `sdl-freerdp` |

### `[remoteapp]`
| Key | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `default_app` | `string` | `"remoteapp-launcher.exe"` | Path to guest agent executable on Windows |

### `[lifecycle]`
| Key | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `idle_disconnect_timeout` | `u64` | `30` | Seconds with no RDP windows before disconnect (0 = disable) |
| `vm_suspend_timeout` | `u64` | `300` | Inactivity seconds before VM suspend (0 = disable) |
| `vm_name` | `string` | `"win11"` | Libvirt domain name |
| `reclaim_virtio_mem` | `bool` | `true` | Deflate virtio-mem dynamic memory before suspend |
| `virtio_mem_alias` | `string` | `"ua-virtiomem0"` | virtio-mem device alias in domain XML |

### `[mounts]`
A map of Windows drive letters to Linux host directories:
```toml
[mounts]
"F:" = "/data/games"
"J:" = "/data/daily_data"
"M:" = "/data/big_file"
"Y:" = "/home/skwj111"
```

### `[[overrides]]`
Rules for matching remote applications to FreeDesktop MIME types and categories:
```toml
[[overrides]]
match = "excel"
mimes = [
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.ms-excel",
    "text/csv"
]
categories = "Office;Spreadsheet;RemoteApp;Network;"

[[overrides]]
match = "word"
mimes = [
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/msword",
    "application/rtf"
]
categories = "Office;WordProcessor;RemoteApp;Network;"

[[overrides]]
match = "powerpoint"
mimes = [
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.ms-powerpoint"
]
categories = "Office;Presentation;RemoteApp;Network;"
```
