mod config;
mod lifecycle;

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use keyring::Entry;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RemoteAppInfo {
    pub id: String,
    pub name: String,
    pub app_type: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_base64: Option<String>,
}

#[derive(Parser, Debug)]
#[command(name = "rdp-launcher", about = "Linux local launcher for FreeRDP with Keyring auto-login, VM lifecycle management, and RemoteApp sync")]
struct Cli {
    /// Path to config file (default: ~/.config/rdp-launcher/config.toml)
    #[arg(short = 'c', long)]
    config: Option<PathBuf>,

    /// Override target RDP host
    #[arg(short = 'H', long)]
    host: Option<String>,

    /// Override target Windows username
    #[arg(short = 'u', long)]
    user: Option<String>,

    /// Override FreeRDP binary path
    #[arg(long)]
    freerdp_bin: Option<String>,

    /// Override scale-desktop value (e.g. 100, 150, 200)
    #[arg(long)]
    scale_desktop: Option<u32>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Show full configuration file in TOML format
    Show,
    /// List all configurable keys, types, current values and descriptions
    List,
    /// Get the value of a specific config key
    Get {
        /// The config key (e.g. 'server.host', 'lifecycle.vm_suspend_timeout')
        key: String,
    },
    /// Set the value of a specific config key
    Set {
        /// The config key (e.g. 'server.host', 'scale_desktop', 'vm_suspend_timeout')
        key: String,
        /// The new value (e.g. '192.168.122.14', '600', 'false')
        value: String,
    },
    /// Print the filesystem path of the configuration file
    Path,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Save or update the Windows password in the system Keyring
    SetPassword {
        /// Read password directly from stdin instead of interactive prompt
        #[arg(long)]
        stdin: bool,

        /// Pass password directly on command line
        #[arg(long)]
        password: Option<String>,
    },

    /// Check if a password exists in Keyring
    Status,

    /// Delete the saved password from Keyring
    ClearPassword,

    /// Show current loaded configuration
    ShowConfig,

    /// View, get, or set configuration values directly from the CLI
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    /// Shorthand to set a config value: rdp-launcher set-config <KEY> <VALUE>
    SetConfig {
        /// Config key (e.g. 'server.host', 'scale_desktop', 'vm_suspend_timeout')
        key: String,
        /// New value
        value: String,
    },

    /// Shorthand to get a config value: rdp-launcher get-config <KEY>
    GetConfig {
        /// Config key (e.g. 'server.host', 'vm_name')
        key: String,
    },

    /// Launch FreeRDP with the daemon or specific application
    Launch {
        /// Application to launch (defaults to config default_app, e.g. remoteapp-launcher.exe daemon)
        app: Option<String>,

        /// Additional arguments to pass to sdl-freerdp
        #[arg(last = true)]
        extra_args: Vec<String>,
    },

    /// Fetch and sync applications from the remote Windows host
    SyncApps {
        /// Also generate .desktop files in ~/.local/share/applications/
        #[arg(long, default_value_t = true)]
        create_desktop_entries: bool,
    },

    /// List previously synced applications
    ListApps,

    /// Run an application on the remote host (auto-resumes VM and starts FreeRDP if needed)
    Run {
        /// Application name, ID, or raw target path / AUMID to run
        target: String,
    },

    /// Stop the remote daemon session and FreeRDP
    StopDaemon,

    /// Run the lifecycle watcher service (manages 30s disconnect, virtio-mem reclaim & 5min suspend)
    Watcher,

    /// Show lifecycle status of VM, SSH, FreeRDP and RemoteApp windows
    LifecycleStatus,
}

fn get_keyring_entry(service: &str, user: &str) -> Result<Entry> {
    Entry::new(service, user).context("Failed to initialize keyring entry")
}

fn read_secret(prompt: &str) -> Result<String> {
    if io::stdin().is_terminal() {
        eprintln!("{}", prompt);
        rpassword::read_password().context("Failed to read password from tty")
    } else {
        let mut line = String::new();
        io::stdin().read_line(&mut line).context("Failed to read password from stdin")?;
        let trimmed = line.trim_end_matches(&['\r', '\n'][..]).to_string();
        if trimmed.is_empty() {
            bail!("Password from stdin cannot be empty");
        }
        Ok(trimmed)
    }
}

fn set_password_interactive(service: &str, user: &str, password_opt: Option<String>, from_stdin: bool) -> Result<String> {
    let pass = if let Some(p) = password_opt {
        p
    } else if from_stdin || !io::stdin().is_terminal() {
        read_secret("")?
    } else {
        let p1 = read_secret(&format!("Enter Windows password for user '{}':", user))?;
        if p1.is_empty() {
            bail!("Password cannot be empty");
        }
        let p2 = read_secret("Confirm password:")?;
        if p1 != p2 {
            bail!("Passwords do not match");
        }
        p1
    };

    let entry = get_keyring_entry(service, user)?;
    entry.set_password(&pass).context("Failed to save password in Keyring")?;
    eprintln!("Password successfully stored in Keyring (service='{}', user='{}')", service, user);
    Ok(pass)
}

fn get_or_prompt_password(service: &str, user: &str) -> Result<String> {
    let entry = get_keyring_entry(service, user)?;
    match entry.get_password() {
        Ok(pass) => Ok(pass),
        Err(keyring::Error::NoEntry) => {
            eprintln!("No password stored in Keyring for '{}'. Let's set it now.", user);
            set_password_interactive(service, user, None, false)
        }
        Err(e) => {
            eprintln!("Warning reading Keyring: {}. Falling back to prompt.", e);
            read_secret(&format!("Enter Windows password for '{}':", user))
        }
    }
}

pub fn get_cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cache").join("rdp-launcher")
}

pub fn get_desktop_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local").join("share").join("applications")
}

pub fn is_freerdp_running() -> bool {
    if let Ok(entries) = fs::read_dir("/proc") {
        let current_pid = std::process::id();
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Ok(pid) = name.to_string_lossy().parse::<u32>() {
                if pid == current_pid {
                    continue;
                }
                let comm_path = entry.path().join("comm");
                if let Ok(comm) = fs::read_to_string(comm_path) {
                    if comm.trim() == "sdl-freerdp" {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn resolve_host_ip(config: &Config) -> String {
    // If user specified an explicit non-auto IP, use it
    if !config.server.host.is_empty() && config.server.host != "auto" {
        return config.server.host.clone();
    }

    // Try detecting IP from VM guest agent or lease
    let ips = lifecycle::get_vm_ips(&config.lifecycle.vm_name);
    for ip in ips {
        if ip.contains('.') {
            return ip;
        }
    }

    config.server.host.clone()
}

fn wait_for_rdp_port(host: &str, timeout_secs: u64) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let addr = format!("{}:3389", host);
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(timeout_secs) {
        if let Ok(mut addrs) = addr.to_socket_addrs() {
            if let Some(target) = addrs.next() {
                if TcpStream::connect_timeout(&target, std::time::Duration::from_millis(800)).is_ok() {
                    // Small grace period for Windows user profile / RPC services to settle
                    std::thread::sleep(std::time::Duration::from_millis(2000));
                    return true;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
    false
}

fn ensure_display_environment() -> Result<()> {
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    let x11_display = std::env::var("DISPLAY").ok();

    if wayland_display.is_none() && x11_display.is_none() {
        eprintln!("\n{}", "=".repeat(72));
        eprintln!("  [Error] No Wayland or X11 display socket detected in current shell!");
        eprintln!("{}", "=".repeat(72));
        eprintln!("  Current environment variables ($WAYLAND_DISPLAY and $DISPLAY) are not set.");
        eprintln!("  You appear to be running inside a headless SSH session, TTY, or background script.");
        eprintln!();
        eprintln!("  Why did this call fail?");
        eprintln!("    RemoteApp (via FreeRDP) creates native graphical Wayland/X11 client windows.");
        eprintln!("    Without an active display server socket connected to this shell session, GUI");
        eprintln!("    windows CANNOT be rendered into a text terminal.");
        eprintln!();
        eprintln!("  How to use RemoteApp:");
        eprintln!("    1. Run directly from within your Wayland graphical desktop (e.g. Niri terminal/dms).");
        eprintln!("    2. If you are connected via SSH and intentionally wish to launch the window");
        eprintln!("       onto the host machine's physical desktop, explicitly specify your display:");
        eprintln!("         WAYLAND_DISPLAY=wayland-1 rdp-launcher run <app>");
        eprintln!("{}\n", "=".repeat(72));
        bail!("Aborted: No Wayland ($WAYLAND_DISPLAY) or X11 ($DISPLAY) socket in current session.");
    }

    // If WAYLAND_DISPLAY is set, verify the socket file actually exists
    if let Some(ref w_disp) = wayland_display {
        let uid = unsafe {
            extern "C" { fn getuid() -> u32; }
            getuid()
        };
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", uid));
        let sock_path = std::path::Path::new(&runtime_dir).join(w_disp);
        if !sock_path.exists() {
            eprintln!("\n{}", "=".repeat(72));
            eprintln!("  [Error] Wayland socket '{}' not found in $XDG_RUNTIME_DIR ({})!", w_disp, runtime_dir);
            eprintln!("{}", "=".repeat(72));
            eprintln!("  The Wayland compositor does not appear to be running or the socket has closed.");
            eprintln!("{}\n", "=".repeat(72));
            bail!("Wayland socket file '{:?}' does not exist.", sock_path);
        }
    }

    Ok(())
}

fn ensure_freerdp_session(config: &Config) -> Result<()> {
    ensure_display_environment()?;

    if is_freerdp_running() {
        return Ok(());
    }

    // Make sure VM is resumed first if it was suspended
    let _ = lifecycle::resume_vm_if_needed(&config.lifecycle.vm_name);

    let host = resolve_host_ip(config);
    println!("[rdp-launcher] Waiting for Windows RDP service on {}:3389 to become reachable...", host);
    if !wait_for_rdp_port(&host, 90) {
        bail!("Timed out waiting for {}:3389 to become reachable", host);
    }
    println!("[rdp-launcher] {}:3389 is online! Starting RemoteApp daemon...", host);

    let freerdp_bin = &config.freerdp.bin;
    if !std::path::Path::new(freerdp_bin).exists() {
        bail!("FreeRDP binary not found at: {}", freerdp_bin);
    }

    let service = &config.server.service;
    let user = &config.server.user;
    let password = get_or_prompt_password(service, user)?;

    let daemon_app = &config.remoteapp.default_app;

    let mut args = vec![
        format!("/v:{}", host),
        format!("/u:{}", user),
        format!("/app:program:\"{}\"", daemon_app),
        "/from-stdin:force".to_string(),
    ];

    if config.freerdp.cert_ignore {
        args.push("/cert:ignore".to_string());
    }
    if config.freerdp.clipboard {
        args.push("+clipboard".to_string());
    }
    if let Some(scale_desktop) = config.freerdp.scale_desktop {
        args.push(format!("/scale-desktop:{}", scale_desktop));
    }
    if let Some(scale) = config.freerdp.scale {
        args.push(format!("/scale:{}", scale));
    }
    if let Some(ref filters) = config.freerdp.log_filters {
        args.push(format!("/log-filters:{}", filters));
    }

    args.extend(config.freerdp.extra_args.clone());

    let cache_dir = get_cache_dir();
    let _ = fs::create_dir_all(&cache_dir);
    let log_path = cache_dir.join("freerdp.log");
    let log_file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open log file at {:?}", log_path))?;

    let mut cmd = Command::new(freerdp_bin);
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file));

    if let Some(ref driver) = config.freerdp.video_driver {
        cmd.env("SDL_VIDEODRIVER", driver);
    }

    // Bypass any local HTTP/SOCKS proxies for direct LAN VM connection
    cmd.env_remove("http_proxy");
    cmd.env_remove("https_proxy");
    cmd.env_remove("all_proxy");
    cmd.env_remove("HTTP_PROXY");
    cmd.env_remove("HTTPS_PROXY");
    cmd.env_remove("ALL_PROXY");
    cmd.env("no_proxy", "*");
    cmd.env("NO_PROXY", "*");

    println!("[rdp-launcher] Spawning FreeRDP in background (logging to {:?})...", log_path);
    let mut child = cmd.spawn()
        .with_context(|| format!("Failed to spawn FreeRDP ({})", freerdp_bin))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(password.as_bytes()).context("Failed to write password to FreeRDP stdin")?;
        stdin.write_all(b"\n").context("Failed to write newline to FreeRDP stdin")?;
        let _ = stdin.flush();
        drop(stdin);
    }

    println!("[rdp-launcher] Awaiting remote daemon initialization...");
    let probe_cmd = format!("pwsh -NoProfile -Command \"& '{}' ping\"", daemon_app);

    let mut ready = false;
    for i in 0..60 {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let out = Command::new("ssh")
            .args(&[format!("{}@{}", user, host), probe_cmd.clone()])
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                ready = true;
                break;
            }
        }
        if (i + 1) % 10 == 0 {
            println!("[rdp-launcher] Still awaiting remote daemon ({}s elapsed)...", i + 1);
        }
    }

    if ready {
        println!("[rdp-launcher] RemoteApp session and daemon pipe ready!");
    } else {
        println!("[rdp-launcher] Daemon initialization taking longer, proceeding...");
    }

    Ok(())
}

fn sync_remote_apps(config: &Config, create_desktop: bool) -> Result<()> {
    let user = &config.server.user;
    let host = resolve_host_ip(config);
    let daemon_app = &config.remoteapp.default_app;

    println!("Fetching installed applications from {}@{} via SSH...", user, host);

    let remote_cmd = format!("pwsh -NoProfile -Command \"& '{}' list-apps --with-icons | Out-String\"", daemon_app);
    let output = Command::new("ssh")
        .args(&[format!("{}@{}", user, host), remote_cmd])
        .output()
        .context("Failed to execute SSH command")?;

    if !output.status.success() {
        bail!("Remote list-apps failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let apps: Vec<RemoteAppInfo> = serde_json::from_slice(&output.stdout)
        .context("Failed to parse JSON response from remote host")?;

    let cache_dir = get_cache_dir();
    let icons_dir = cache_dir.join("icons");
    fs::create_dir_all(&icons_dir).context("Failed to create icons cache directory")?;

    let apps_cache_file = cache_dir.join("apps.json");
    fs::write(&apps_cache_file, &output.stdout).context("Failed to write apps cache file")?;

    let desktop_dir = get_desktop_dir();
    if create_desktop {
        let _ = fs::create_dir_all(&desktop_dir);
    }

    let self_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("rdp-launcher"));

    println!("Synchronized {} applications:", apps.len());
    for app in &apps {
        let mut icon_path_str = String::new();
        if let Some(ref b64) = app.icon_base64 {
            if let Ok(bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.trim()) {
                let icon_file = icons_dir.join(format!("{}.png", app.id));
                if fs::write(&icon_file, bytes).is_ok() {
                    icon_path_str = icon_file.to_string_lossy().into_owned();
                }
            }
        }

        if create_desktop {
            let desktop_file = desktop_dir.join(format!("remoteapp-{}.desktop", app.id));
            let icon_entry = if !icon_path_str.is_empty() {
                format!("Icon={}\n", icon_path_str)
            } else {
                String::new()
            };

            let entry_content = format!(
                "[Desktop Entry]\n\
                 Version=1.0\n\
                 Type=Application\n\
                 Name={} (Remote)\n\
                 Comment=RemoteApp on {}\n\
                 Exec={} run {}\n\
                 {}Terminal=false\n\
                 Categories=RemoteApp;Network;\n",
                app.name,
                host,
                self_exe.display(),
                app.id,
                icon_entry
            );

            let _ = fs::write(&desktop_file, entry_content);
        }

        println!("  - {:<28} [{:<5}] -> {}", app.name, app.app_type, app.target);
    }

    println!("\nCache saved to: {:?}", apps_cache_file);
    if create_desktop {
        let _ = Command::new("update-desktop-database").arg(&desktop_dir).status();
        println!(".desktop entries generated and database updated in: {:?}", desktop_dir);
    }

    Ok(())
}

fn list_cached_apps() -> Result<()> {
    let apps_cache_file = get_cache_dir().join("apps.json");
    if !apps_cache_file.exists() {
        println!("No cached applications found. Run 'rdp-launcher sync-apps' first.");
        return Ok(());
    }

    let data = fs::read_to_string(&apps_cache_file)?;
    let apps: Vec<RemoteAppInfo> = serde_json::from_str(&data)?;

    println!("{:<20} {:<30} {:<6} {}", "ID", "NAME", "TYPE", "TARGET");
    println!("{}", "-".repeat(80));
    for app in apps {
        println!("{:<20} {:<30} {:<6} {}", app.id, app.name, app.app_type, app.target);
    }

    Ok(())
}

fn run_remote_target(config: &Config, target: &str) -> Result<()> {
    // 1. Ensure FreeRDP session is running (and VM is resumed if needed)
    ensure_freerdp_session(config)?;

    // 2. Resolve target if it matches a cached Name or ID
    let resolved_target = if let Ok(data) = fs::read_to_string(get_cache_dir().join("apps.json")) {
        if let Ok(apps) = serde_json::from_str::<Vec<RemoteAppInfo>>(&data) {
            if let Some(app) = apps.iter().find(|a| {
                a.id.eq_ignore_ascii_case(target) 
                || a.name.eq_ignore_ascii_case(target)
                || (target.eq_ignore_ascii_case("notepad") && (a.name == "记事本" || a.target.contains("Notepad")))
                || (target.eq_ignore_ascii_case("calc") && (a.name == "计算器" || a.target.contains("Calculator")))
                || (target.eq_ignore_ascii_case("terminal") && (a.name == "终端" || a.target.contains("Terminal")))
            }) {
                app.target.clone()
            } else {
                target.to_string()
            }
        } else {
            target.to_string()
        }
    } else {
        target.to_string()
    };

    let user = &config.server.user;
    let host = resolve_host_ip(config);
    let daemon_app = &config.remoteapp.default_app;
    println!("[rdp-launcher] Requesting remote host {}@{} to launch '{}'...", user, host, resolved_target);

    let remote_cmd = format!("pwsh -NoProfile -Command \"& '{}' run '{}'\"", daemon_app, resolved_target);
    let status = Command::new("ssh")
        .args(&[format!("{}@{}", user, host), remote_cmd])
        .status()
        .context("Failed to send run command via SSH")?;

    if !status.success() {
        bail!("Remote execution failed");
    }

    Ok(())
}

pub fn stop_remote_daemon(config: &Config) -> Result<()> {
    let user = &config.server.user;
    let host = resolve_host_ip(config);
    let daemon_app = &config.remoteapp.default_app;

    println!("Stopping RemoteApp daemon on {}@{}...", user, host);
    let remote_cmd = format!("pwsh -NoProfile -Command \"& '{}' stop\"", daemon_app);
    let _ = Command::new("ssh")
        .args(&[format!("{}@{}", user, host), remote_cmd])
        .status();

    let _ = Command::new("pkill").arg("-x").arg("sdl-freerdp").status();

    println!("Session stopped.");
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (mut config, config_path) = Config::load_or_create(cli.config.as_deref())?;

    if let Some(host) = cli.host {
        config.server.host = host;
    }
    if let Some(user) = cli.user {
        config.server.user = user;
    }
    if let Some(bin) = cli.freerdp_bin {
        config.freerdp.bin = bin;
    }
    if let Some(scale) = cli.scale_desktop {
        config.freerdp.scale_desktop = Some(scale);
    }

    let service = &config.server.service;
    let user = &config.server.user;

    let command = cli.command.unwrap_or(Commands::Launch {
        app: None,
        extra_args: vec![],
    });

    match command {
        Commands::SetPassword { stdin, password } => {
            set_password_interactive(service, user, password, stdin)?;
        }
        Commands::Status => {
            let entry = get_keyring_entry(service, user)?;
            match entry.get_password() {
                Ok(_) => println!("Password is saved in Keyring for service='{}', user='{}'.", service, user),
                Err(keyring::Error::NoEntry) => println!("No password found in Keyring for service='{}', user='{}'.", service, user),
                Err(e) => println!("Keyring error: {}", e),
            }
        }
        Commands::ClearPassword => {
            let entry = get_keyring_entry(service, user)?;
            match entry.delete_credential() {
                Ok(_) => println!("Password cleared from Keyring for service='{}', user='{}'.", service, user),
                Err(keyring::Error::NoEntry) => println!("No password was saved for service='{}', user='{}'.", service, user),
                Err(e) => bail!("Failed to delete password: {}", e),
            }
        }
        Commands::ShowConfig => {
            println!("Loaded config from {:?}:\n", config_path);
            let toml_str = toml::to_string_pretty(&config)?;
            println!("{}", toml_str);
        }
        Commands::GetConfig { key } => {
            let val = config.get_key(&key)?;
            println!("{}", val);
        }
        Commands::SetConfig { key, value } => {
            let old_val = config.set_key(&key, &value)?;
            config.save(&config_path)?;
            println!("Updated '{}': '{}' -> '{}'", key, old_val, value);
            println!("Saved changes to {:?}", config_path);
        }
        Commands::Config { action } => {
            match action.unwrap_or(ConfigAction::List) {
                ConfigAction::Show => {
                    println!("Configuration file: {:?}\n", config_path);
                    let toml_str = toml::to_string_pretty(&config)?;
                    println!("{}", toml_str);
                }
                ConfigAction::Path => {
                    println!("{:?}", config_path);
                }
                ConfigAction::List => {
                    println!("Configuration File: {:?}\n", config_path);
                    println!("{:<36} {:<14} {:<24} {}", "KEY", "TYPE", "CURRENT VALUE", "DESCRIPTION");
                    println!("{}", "-".repeat(110));
                    for (k, t, val, desc) in config.list_keys() {
                        println!("{:<36} {:<14} {:<24} {}", k, t, val, desc);
                    }
                    println!("\nUsage:");
                    println!("  rdp-launcher config set <KEY> <VALUE>    # Update a configuration value");
                    println!("  rdp-launcher config get <KEY>            # Retrieve a configuration value");
                    println!("  rdp-launcher config show                 # Display raw TOML format");
                }
                ConfigAction::Get { key } => {
                    let val = config.get_key(&key)?;
                    println!("{}", val);
                }
                ConfigAction::Set { key, value } => {
                    let old_val = config.set_key(&key, &value)?;
                    config.save(&config_path)?;
                    println!("Successfully updated '{}': '{}' -> '{}'", key, old_val, value);
                    println!("Saved changes to {:?}", config_path);
                }
            }
        }
        Commands::SyncApps { create_desktop_entries } => {
            sync_remote_apps(&config, create_desktop_entries)?;
        }
        Commands::ListApps => {
            list_cached_apps()?;
        }
        Commands::Run { target } => {
            run_remote_target(&config, &target)?;
        }
        Commands::StopDaemon => {
            stop_remote_daemon(&config)?;
        }
        Commands::Watcher => {
            lifecycle::run_watcher(config)?;
        }
        Commands::LifecycleStatus => {
            lifecycle::show_lifecycle_status(&config)?;
        }
        Commands::Launch { app, extra_args } => {
            ensure_display_environment()?;

            let freerdp_bin = &config.freerdp.bin;
            if !std::path::Path::new(freerdp_bin).exists() {
                bail!("FreeRDP binary not found at: {}", freerdp_bin);
            }

            let app_to_launch = app.unwrap_or_else(|| config.remoteapp.default_app.clone());
            let password = get_or_prompt_password(service, user)?;
            let resolved_host = resolve_host_ip(&config);

            println!("Launching RemoteApp '{}' on {}@{}...", app_to_launch, user, resolved_host);

            let mut args = vec![
                format!("/v:{}", resolved_host),
                format!("/u:{}", user),
                format!("/app:program:\"{}\"", app_to_launch),
                "/from-stdin:force".to_string(),
            ];

            if config.freerdp.cert_ignore {
                args.push("/cert:ignore".to_string());
            }
            if config.freerdp.clipboard {
                args.push("+clipboard".to_string());
            }
            if let Some(scale_desktop) = config.freerdp.scale_desktop {
                args.push(format!("/scale-desktop:{}", scale_desktop));
            }
            if let Some(scale) = config.freerdp.scale {
                args.push(format!("/scale:{}", scale));
            }
            if let Some(ref filters) = config.freerdp.log_filters {
                args.push(format!("/log-filters:{}", filters));
            }

            args.extend(config.freerdp.extra_args.clone());
            args.extend(extra_args);

            let mut cmd = Command::new(freerdp_bin);
            cmd.args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());

            if let Some(ref driver) = config.freerdp.video_driver {
                cmd.env("SDL_VIDEODRIVER", driver);
            }

            cmd.env_remove("http_proxy");
            cmd.env_remove("https_proxy");
            cmd.env_remove("all_proxy");
            cmd.env_remove("HTTP_PROXY");
            cmd.env_remove("HTTPS_PROXY");
            cmd.env_remove("ALL_PROXY");
            cmd.env("no_proxy", "*");
            cmd.env("NO_PROXY", "*");

            println!("Spawning: SDL_VIDEODRIVER={:?} {} {}", 
                     config.freerdp.video_driver.as_deref().unwrap_or("<default>"),
                     freerdp_bin, 
                     args.join(" "));

            let mut child = cmd.spawn()
                .with_context(|| format!("Failed to spawn FreeRDP ({})", freerdp_bin))?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(password.as_bytes()).context("Failed to write password to FreeRDP stdin")?;
                stdin.write_all(b"\n").context("Failed to write newline to FreeRDP stdin")?;
                let _ = stdin.flush();
                drop(stdin);
            }

            let status = child.wait().context("Failed to wait on FreeRDP process")?;
            if !status.success() {
                eprintln!("FreeRDP exited with code: {:?}", status.code());
            }
        }
    }

    Ok(())
}
