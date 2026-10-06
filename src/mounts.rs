use std::collections::HashMap;
use std::fs;
use std::path::Path;
use anyhow::{Context, Result, bail};

use crate::config::Config;

/// Parses Samba configuration file (INI format) to extract share name -> host path mappings
pub fn parse_samba_shares(conf_path: &Path) -> Result<HashMap<String, String>> {
    if !conf_path.exists() {
        bail!("Samba configuration file not found at: {:?}", conf_path);
    }

    let content = fs::read_to_string(conf_path)
        .with_context(|| format!("Failed to read Samba config at {:?}", conf_path))?;

    let mut shares = HashMap::new();
    let mut current_section: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with(';') || trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed[1..trimmed.len() - 1].trim().to_string();
            current_section = Some(section);
            continue;
        }

        if let Some(ref section) = current_section {
            if section.eq_ignore_ascii_case("global") {
                continue;
            }

            if let Some((key, val)) = trimmed.split_once('=') {
                let k = key.trim().to_lowercase();
                let v = val.trim();
                if k == "path" && !v.is_empty() {
                    shares.insert(section.clone(), v.to_string());
                }
            }
        }
    }

    Ok(shares)
}

/// Synchronizes the mount table by correlating Samba shares with Windows mapped drives
pub fn sync_mount_table(config: &mut Config) -> Result<Vec<(String, String, String)>> {
    let conf_path_str = config.samba.config_path.as_deref().unwrap_or("/etc/samba/smb-win11.conf");
    let conf_path = Path::new(conf_path_str);

    let shares = parse_samba_shares(conf_path)
        .with_context(|| format!("Failed to parse Samba configuration from {:?}", conf_path))?;

    // Query Windows network drives directly via TCP Agent (Zero SSH, sub-millisecond)
    let client = crate::client::get_or_ensure_client(config)?;
    let win_drives = client.get_mapped_drives()?;

    let mut synced = Vec::new();

    for (drive, provider) in win_drives {
        let clean_provider = provider.replace('/', "\\");
        let share_name = clean_provider.trim_end_matches('\\').split('\\').next_back().unwrap_or("");
        
        // Find matching Samba share
        if let Some(host_path) = shares.get(share_name) {
            let drive_key = if drive.ends_with(':') { drive.clone() } else { format!("{}:", drive) };
            config.mounts.insert(drive_key.clone(), host_path.clone());
            synced.push((drive_key, share_name.to_string(), host_path.clone()));
        }
    }

    Ok(synced)
}

/// Translates a Linux file/directory path to the corresponding Windows guest path
pub fn path_to_windows(config: &Config, linux_path_str: &str) -> Result<String> {
    let p = Path::new(linux_path_str);
    let abs_path = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };

    // Canonicalize if file exists
    let canonical = abs_path.canonicalize().unwrap_or(abs_path);
    let canonical_str = canonical.to_string_lossy().to_string();

    let mut best_match: Option<(&String, &String)> = None;
    for (win_drive, host_path) in &config.mounts {
        let clean_host = host_path.trim_end_matches('/');
        if canonical_str == clean_host || canonical_str.starts_with(&format!("{}/", clean_host)) {
            match best_match {
                None => best_match = Some((win_drive, host_path)),
                Some((_, prev_host)) => {
                    if host_path.len() > prev_host.len() {
                        best_match = Some((win_drive, host_path));
                    }
                }
            }
        }
    }

    if let Some((win_drive, host_path)) = best_match {
        let clean_host = host_path.trim_end_matches('/');
        let remainder = &canonical_str[clean_host.len()..];
        let win_remainder = remainder.replace('/', "\\");
        
        let drive_prefix = win_drive.trim_end_matches('\\');
        if win_remainder.is_empty() || win_remainder == "\\" {
            Ok(format!("{}\\", drive_prefix))
        } else if win_remainder.starts_with('\\') {
            Ok(format!("{}{}", drive_prefix, win_remainder))
        } else {
            Ok(format!("{}\\{}", drive_prefix, win_remainder))
        }
    } else {
        let mut msg = format!("Path '{}' is not located within any configured mount.\n\nConfigured Mounts:\n", canonical_str);
        if config.mounts.is_empty() {
            msg.push_str("  (No mounts configured. Run 'rdp-launcher mounts sync' to detect them from Samba.)\n");
        } else {
            for (d, h) in &config.mounts {
                msg.push_str(&format!("  {} <=> {}\n", d, h));
            }
        }
        bail!(msg);
    }
}

/// Translates a Windows guest path to the corresponding Linux host path
pub fn path_to_linux(config: &Config, win_path_str: &str) -> Result<String> {
    let trimmed = win_path_str.trim();
    if trimmed.len() < 2 || !trimmed.chars().nth(1).map(|c| c == ':').unwrap_or(false) {
        bail!("Invalid Windows path: '{}'. Expected drive letter like 'X:\\...'.", win_path_str);
    }

    let drive = format!("{}:", trimmed[..1].to_ascii_uppercase());
    let remainder = if trimmed.len() > 2 {
        &trimmed[2..]
    } else {
        ""
    };

    if let Some(host_path) = config.mounts.get(&drive) {
        let linux_remainder = remainder.replace('\\', "/");
        let clean_host = host_path.trim_end_matches('/');
        if linux_remainder.is_empty() || linux_remainder == "/" {
            Ok(clean_host.to_string())
        } else if linux_remainder.starts_with('/') {
            Ok(format!("{}{}", clean_host, linux_remainder))
        } else {
            Ok(format!("{}/{}", clean_host, linux_remainder))
        }
    } else {
        bail!("Windows drive '{}' is not mapped to any Linux host directory in config.", drive);
    }
}

/// Displays the current mount table
pub fn show_mounts(config: &Config) {
    println!("=== RDP & Windows Mount Table ===");
    let samba_path = config.samba.config_path.as_deref().unwrap_or("auto-detect");
    println!("Samba Config Source: {}\n", samba_path);

    if config.mounts.is_empty() {
        println!("No active mounts configured.");
        println!("Run 'rdp-launcher mounts sync' to automatically detect mounts from Samba and Windows.\n");
        return;
    }

    println!("{:<18} {:<36} STATUS", "WINDOWS DRIVE", "LINUX HOST PATH");
    println!("{}", "-".repeat(75));

    let mut keys: Vec<&String> = config.mounts.keys().collect();
    keys.sort();

    for k in keys {
        let host_path = &config.mounts[k];
        let p = Path::new(host_path);
        let status = if p.is_dir() {
            "Directory Exists"
        } else if p.exists() {
            "File Exists"
        } else {
            "Host Path Missing (!)"
        };
        println!("{:<18} {:<36} {}", format!("{}\\", k.trim_end_matches('\\')), host_path, status);
    }

    println!("\nUsage:");
    println!("  rdp-launcher mounts sync       # Sync mappings from Samba & Windows");
    println!("  rdp-launcher path <PATH>       # Translate path between Linux and Windows");
    println!("  rdp-launcher open <FILE>       # Open Linux file in Windows 365 / RemoteApp");
}
