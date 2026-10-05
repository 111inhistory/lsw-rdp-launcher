use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub freerdp: FreeRdpConfig,
    #[serde(default)]
    pub remoteapp: RemoteAppConfig,
    #[serde(default)]
    pub lifecycle: LifecycleConfig,
    #[serde(default)]
    pub mounts: HashMap<String, String>,
    #[serde(default)]
    pub samba: SambaConfig,
    #[serde(default = "default_overrides")]
    pub overrides: Vec<AppOverride>,
}

pub fn default_overrides() -> Vec<AppOverride> {
    vec![
        AppOverride {
            match_pattern: "excel".to_string(),
            mimes: vec![
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string(),
                "application/vnd.ms-excel".to_string(),
                "text/csv".to_string(),
            ],
            categories: Some("Office;Spreadsheet;RemoteApp;Network;".to_string()),
        },
        AppOverride {
            match_pattern: "word".to_string(),
            mimes: vec![
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
                "application/msword".to_string(),
                "application/rtf".to_string(),
            ],
            categories: Some("Office;WordProcessor;RemoteApp;Network;".to_string()),
        },
        AppOverride {
            match_pattern: "powerpoint".to_string(),
            mimes: vec![
                "application/vnd.openxmlformats-officedocument.presentationml.presentation".to_string(),
                "application/vnd.ms-powerpoint".to_string(),
            ],
            categories: Some("Office;Presentation;RemoteApp;Network;".to_string()),
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppOverride {
    #[serde(rename = "match")]
    pub match_pattern: String,
    #[serde(default)]
    pub mimes: Vec<String>,
    #[serde(default)]
    pub categories: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SambaConfig {
    pub config_path: Option<String>,
}

impl Default for SambaConfig {
    fn default() -> Self {
        let default_path = if Path::new("/etc/samba/smb-win11.conf").exists() {
            Some("/etc/samba/smb-win11.conf".to_string())
        } else if Path::new("/etc/samba/smb.conf").exists() {
            Some("/etc/samba/smb.conf".to_string())
        } else {
            None
        };
        Self {
            config_path: default_path,
        }
    }
}

fn default_agent_port() -> u16 {
    49152
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub user: String,
    pub service: String,
    #[serde(default = "default_agent_port")]
    pub agent_port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let default_user = std::env::var("USER").unwrap_or_else(|_| "Administrator".to_string());
        Self {
            host: "auto".to_string(),
            user: default_user,
            service: "rdp-bridge".to_string(),
            agent_port: 49152,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreeRdpConfig {
    pub bin: String,
    pub video_driver: Option<String>,
    pub scale_desktop: Option<u32>,
    pub scale: Option<u32>,
    pub clipboard: bool,
    pub cert_ignore: bool,
    pub log_filters: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

pub fn find_default_freerdp_bin() -> String {
    // 1. Check FREERDP_BIN environment variable
    if let Ok(env_bin) = std::env::var("FREERDP_BIN") {
        if std::path::Path::new(&env_bin).is_file() {
            return env_bin;
        }
    }
    // 2. Check PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("sdl-freerdp");
            if candidate.is_file() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }
    // 3. Check standard system locations
    for candidate in &[
        "/usr/local/bin/sdl-freerdp",
        "/usr/bin/sdl-freerdp",
    ] {
        if std::path::Path::new(candidate).is_file() {
            return candidate.to_string();
        }
    }
    // 4. Generic command fallback
    "sdl-freerdp".to_string()
}

impl Default for FreeRdpConfig {
    fn default() -> Self {
        Self {
            bin: find_default_freerdp_bin(),
            video_driver: Some("wayland".to_string()),
            scale_desktop: Some(200),
            scale: None,
            clipboard: true,
            cert_ignore: true,
            log_filters: Some("com.freerdp.client.sdl*:DEBUG".to_string()),
            extra_args: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAppConfig {
    pub default_app: String,
}

impl Default for RemoteAppConfig {
    fn default() -> Self {
        Self {
            default_app: "remoteapp-launcher.exe".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleConfig {
    /// Seconds with no RemoteApp windows before disconnecting FreeRDP (0 = disabled)
    pub idle_disconnect_timeout: u64,
    /// Seconds of VM idle (no FreeRDP, no SSH) before suspending VM (0 = disabled)
    pub vm_suspend_timeout: u64,
    /// Name of the libvirt virtual machine domain
    pub vm_name: String,
    /// Whether to reclaim virtio-mem dynamic memory before suspending
    pub reclaim_virtio_mem: bool,
    /// The virtio-mem alias in libvirt domain XML
    pub virtio_mem_alias: String,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            idle_disconnect_timeout: 30,
            vm_suspend_timeout: 300,
            vm_name: "win11".to_string(),
            reclaim_virtio_mem: true,
            virtio_mem_alias: "ua-virtiomem0".to_string(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            freerdp: FreeRdpConfig::default(),
            remoteapp: RemoteAppConfig::default(),
            lifecycle: LifecycleConfig::default(),
            mounts: HashMap::new(),
            samba: SambaConfig::default(),
            overrides: default_overrides(),
        }
    }
}

pub fn default_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("rdp-launcher").join("config.toml");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join("rdp-launcher").join("config.toml");
    }
    PathBuf::from("config.toml")
}

fn parse_bool(val: &str) -> Result<bool> {
    let lower = val.trim().to_lowercase();
    match lower.as_str() {
        "true" | "1" | "yes" | "on" | "enable" | "enabled" => Ok(true),
        "false" | "0" | "no" | "off" | "disable" | "disabled" => Ok(false),
        _ => anyhow::bail!("Invalid boolean value '{}'. Use true/false, yes/no, on/off, or 1/0.", val),
    }
}

impl Config {
    pub fn save(&self, path: &Path) -> Result<()> {
        let toml_str = toml::to_string_pretty(self)
            .context("Failed to serialize config to TOML")?;
        let header = "# rdp-launcher configuration\n\n";
        let full = format!("{}{}", header, toml_str);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(path, full)
            .with_context(|| format!("Failed to write config file to {:?}", path))?;
        Ok(())
    }

    pub fn set_key(&mut self, key: &str, val: &str) -> Result<String> {
        let k = key.to_lowercase().replace('-', "_");
        let old_val;
        match k.as_str() {
            "server.host" | "host" => {
                old_val = self.server.host.clone();
                self.server.host = val.to_string();
            }
            "server.user" | "user" => {
                old_val = self.server.user.clone();
                self.server.user = val.to_string();
            }
            "server.agent_port" | "agent_port" | "port" => {
                old_val = self.server.agent_port.to_string();
                let p: u16 = val.parse().context("Invalid port number")?;
                self.server.agent_port = p;
            }
            "server.service" | "service" => {
                old_val = self.server.service.clone();
                self.server.service = val.to_string();
            }
            "freerdp.bin" | "freerdp_bin" | "bin" => {
                old_val = self.freerdp.bin.clone();
                self.freerdp.bin = val.to_string();
            }
            "freerdp.video_driver" | "video_driver" => {
                old_val = self.freerdp.video_driver.clone().unwrap_or_else(|| "none".to_string());
                if val.eq_ignore_ascii_case("none") || val.eq_ignore_ascii_case("null") || val.is_empty() {
                    self.freerdp.video_driver = None;
                } else {
                    self.freerdp.video_driver = Some(val.to_string());
                }
            }
            "freerdp.scale_desktop" | "scale_desktop" => {
                old_val = self.freerdp.scale_desktop.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string());
                if val.eq_ignore_ascii_case("none") || val.eq_ignore_ascii_case("null") {
                    self.freerdp.scale_desktop = None;
                } else {
                    let num: u32 = val.parse().context("Invalid number for scale_desktop")?;
                    self.freerdp.scale_desktop = Some(num);
                }
            }
            "freerdp.scale" | "scale" => {
                old_val = self.freerdp.scale.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string());
                if val.eq_ignore_ascii_case("none") || val.eq_ignore_ascii_case("null") {
                    self.freerdp.scale = None;
                } else {
                    let num: u32 = val.parse().context("Invalid number for scale")?;
                    self.freerdp.scale = Some(num);
                }
            }
            "freerdp.clipboard" | "clipboard" => {
                old_val = self.freerdp.clipboard.to_string();
                self.freerdp.clipboard = parse_bool(val)?;
            }
            "freerdp.cert_ignore" | "cert_ignore" => {
                old_val = self.freerdp.cert_ignore.to_string();
                self.freerdp.cert_ignore = parse_bool(val)?;
            }
            "freerdp.log_filters" | "log_filters" => {
                old_val = self.freerdp.log_filters.clone().unwrap_or_else(|| "none".to_string());
                if val.eq_ignore_ascii_case("none") || val.eq_ignore_ascii_case("null") {
                    self.freerdp.log_filters = None;
                } else {
                    self.freerdp.log_filters = Some(val.to_string());
                }
            }
            "remoteapp.default_app" | "default_app" => {
                old_val = self.remoteapp.default_app.clone();
                self.remoteapp.default_app = val.to_string();
            }
            "lifecycle.idle_disconnect_timeout" | "idle_disconnect_timeout" | "idle_disconnect" => {
                old_val = self.lifecycle.idle_disconnect_timeout.to_string();
                let num: u64 = val.parse().context("Invalid integer for idle_disconnect_timeout")?;
                self.lifecycle.idle_disconnect_timeout = num;
            }
            "lifecycle.vm_suspend_timeout" | "vm_suspend_timeout" | "vm_suspend" => {
                old_val = self.lifecycle.vm_suspend_timeout.to_string();
                let num: u64 = val.parse().context("Invalid integer for vm_suspend_timeout")?;
                self.lifecycle.vm_suspend_timeout = num;
            }
            "lifecycle.vm_name" | "vm_name" => {
                old_val = self.lifecycle.vm_name.clone();
                self.lifecycle.vm_name = val.to_string();
            }
            "lifecycle.reclaim_virtio_mem" | "reclaim_virtio_mem" => {
                old_val = self.lifecycle.reclaim_virtio_mem.to_string();
                self.lifecycle.reclaim_virtio_mem = parse_bool(val)?;
            }
            "lifecycle.virtio_mem_alias" | "virtio_mem_alias" => {
                old_val = self.lifecycle.virtio_mem_alias.clone();
                self.lifecycle.virtio_mem_alias = val.to_string();
            }
            "samba.config_path" | "samba_config" | "samba_conf" => {
                old_val = self.samba.config_path.clone().unwrap_or_else(|| "none".to_string());
                if val.eq_ignore_ascii_case("none") || val.eq_ignore_ascii_case("null") {
                    self.samba.config_path = None;
                } else {
                    self.samba.config_path = Some(val.to_string());
                }
            }
            _ if k.starts_with("mounts.") => {
                let drive = k.strip_prefix("mounts.").unwrap().to_uppercase();
                let drive_key = if drive.ends_with(':') { drive } else { format!("{}:", drive) };
                old_val = self.mounts.get(&drive_key).cloned().unwrap_or_else(|| "none".to_string());
                if val.eq_ignore_ascii_case("none") || val.eq_ignore_ascii_case("null") {
                    self.mounts.remove(&drive_key);
                } else {
                    self.mounts.insert(drive_key, val.to_string());
                }
            }
            _ => {
                anyhow::bail!("Unknown config key '{}'. Run 'rdp-launcher config list' to see all valid keys.", key);
            }
        }
        Ok(old_val)
    }

    pub fn get_key(&self, key: &str) -> Result<String> {
        let k = key.to_lowercase().replace('-', "_");
        let val = match k.as_str() {
            "server.host" | "host" => self.server.host.clone(),
            "server.user" | "user" => self.server.user.clone(),
            "server.agent_port" | "agent_port" | "port" => self.server.agent_port.to_string(),
            "server.service" | "service" => self.server.service.clone(),
            "freerdp.bin" | "freerdp_bin" | "bin" => self.freerdp.bin.clone(),
            "freerdp.video_driver" | "video_driver" => self.freerdp.video_driver.as_deref().unwrap_or("none").to_string(),
            "freerdp.scale_desktop" | "scale_desktop" => self.freerdp.scale_desktop.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string()),
            "freerdp.scale" | "scale" => self.freerdp.scale.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string()),
            "freerdp.clipboard" | "clipboard" => self.freerdp.clipboard.to_string(),
            "freerdp.cert_ignore" | "cert_ignore" => self.freerdp.cert_ignore.to_string(),
            "freerdp.log_filters" | "log_filters" => self.freerdp.log_filters.as_deref().unwrap_or("none").to_string(),
            "remoteapp.default_app" | "default_app" => self.remoteapp.default_app.clone(),
            "lifecycle.idle_disconnect_timeout" | "idle_disconnect_timeout" | "idle_disconnect" => self.lifecycle.idle_disconnect_timeout.to_string(),
            "lifecycle.vm_suspend_timeout" | "vm_suspend_timeout" | "vm_suspend" => self.lifecycle.vm_suspend_timeout.to_string(),
            "lifecycle.vm_name" | "vm_name" => self.lifecycle.vm_name.clone(),
            "lifecycle.reclaim_virtio_mem" | "reclaim_virtio_mem" => self.lifecycle.reclaim_virtio_mem.to_string(),
            "lifecycle.virtio_mem_alias" | "virtio_mem_alias" => self.lifecycle.virtio_mem_alias.clone(),
            "samba.config_path" | "samba_config" | "samba_conf" => self.samba.config_path.as_deref().unwrap_or("none").to_string(),
            _ if k.starts_with("mounts.") => {
                let drive = k.strip_prefix("mounts.").unwrap().to_uppercase();
                let drive_key = if drive.ends_with(':') { drive } else { format!("{}:", drive) };
                self.mounts.get(&drive_key).cloned().unwrap_or_else(|| "none".to_string())
            }
            _ => {
                anyhow::bail!("Unknown config key '{}'. Run 'rdp-launcher config list' to see all valid keys.", key);
            }
        };
        Ok(val)
    }

    pub fn list_keys(&self) -> Vec<(&'static str, &'static str, String, &'static str)> {
        vec![
            ("server.host", "string", self.server.host.clone(), "Windows VM hostname or static IP"),
            ("server.user", "string", self.server.user.clone(), "Windows RDP username"),
            ("server.agent_port", "u16", self.server.agent_port.to_string(), "TCP port of the Windows remoteapp-launcher agent"),
            ("server.service", "string", self.server.service.clone(), "Keyring service name for credentials"),
            ("freerdp.bin", "path", self.freerdp.bin.clone(), "Path to sdl-freerdp binary"),
            ("freerdp.video_driver", "option<str>", self.freerdp.video_driver.as_deref().unwrap_or("none").to_string(), "SDL video driver ('wayland', 'x11', or none)"),
            ("freerdp.scale_desktop", "option<u32>", self.freerdp.scale_desktop.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string()), "FreeRDP desktop scale percentage (e.g. 200)"),
            ("freerdp.scale", "option<u32>", self.freerdp.scale.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string()), "FreeRDP UI scale percentage (e.g. 140)"),
            ("freerdp.clipboard", "bool", self.freerdp.clipboard.to_string(), "Enable clipboard synchronization"),
            ("freerdp.cert_ignore", "bool", self.freerdp.cert_ignore.to_string(), "Ignore self-signed SSL/TLS certificates"),
            ("freerdp.log_filters", "option<str>", self.freerdp.log_filters.as_deref().unwrap_or("none").to_string(), "FreeRDP log filters string"),
            ("remoteapp.default_app", "path", self.remoteapp.default_app.clone(), "Default Windows agent daemon path"),
            ("lifecycle.idle_disconnect_timeout", "u64", self.lifecycle.idle_disconnect_timeout.to_string(), "Seconds before idle FreeRDP disconnects (0 = disable)"),
            ("lifecycle.vm_suspend_timeout", "u64", self.lifecycle.vm_suspend_timeout.to_string(), "Seconds before idle VM suspends (0 = disable)"),
            ("lifecycle.vm_name", "string", self.lifecycle.vm_name.clone(), "Libvirt virtual machine domain name"),
            ("lifecycle.reclaim_virtio_mem", "bool", self.lifecycle.reclaim_virtio_mem.to_string(), "Reclaim virtio-mem before VM suspend"),
            ("lifecycle.virtio_mem_alias", "string", self.lifecycle.virtio_mem_alias.clone(), "Alias of virtio-mem device in libvirt XML"),
            ("samba.config_path", "option<path>", self.samba.config_path.as_deref().unwrap_or("none").to_string(), "Path to Samba server smb.conf file"),
        ]
    }

    pub fn load_or_create(custom_path: Option<&Path>) -> Result<(Self, PathBuf)> {
        let path = match custom_path {
            Some(p) => p.to_path_buf(),
            None => default_config_path(),
        };

        if path.exists() {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read config file at {:?}", path))?;
            let cfg: Config = toml::from_str(&content)
                .with_context(|| format!("Failed to parse TOML config at {:?}", path))?;
            return Ok((cfg, path));
        }

        let default_cfg = Config::default();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let toml_str = toml::to_string_pretty(&default_cfg)
            .context("Failed to serialize default config to TOML")?;
        
        let header = "# rdp-launcher configuration\n# Auto-generated default configuration\n\n";
        let full_content = format!("{}{}", header, toml_str);
        let _ = fs::write(&path, full_content);

        Ok((default_cfg, path))
    }
}
