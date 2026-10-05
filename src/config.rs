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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub user: String,
    pub service: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "192.168.122.14".to_string(),
            user: "skwj111".to_string(),
            service: "rdp-bridge".to_string(),
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

impl Default for FreeRdpConfig {
    fn default() -> Self {
        Self {
            bin: "/code/freerdp/build/client/SDL/SDL3/sdl-freerdp".to_string(),
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
            default_app: "E:\\LanguageSpecific\\Rust\\remoteapp-launcher\\target\\release\\remoteapp-launcher.exe".to_string(),
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

impl Config {
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
