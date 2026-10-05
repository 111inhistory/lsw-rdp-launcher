use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::Config;

#[derive(Deserialize, Debug)]
struct WinLogicalDisk {
    #[serde(rename = "DeviceID")]
    pub device_id: String,
    #[serde(rename = "ProviderName")]
    pub provider_name: Option<String>,
}

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

/// Queries Windows guest mapped network drives (DriveType=4) via WMI / PowerShell
pub fn query_windows_mapped_drives(user: &str, host: &str) -> Result<Vec<(String, String)>> {
    let ps_cmd = "Get-CimInstance Win32_LogicalDisk | Where-Object DriveType -eq 4 | Select-Object DeviceID, ProviderName | ConvertTo-Json -Compress";
    let remote_cmd = format!("powershell -NoProfile -Command \"{}\"", ps_cmd);

    let output = Command::new("ssh")
        .args(&[format!("{}@{}", user, host), remote_cmd])
        .output()
        .context("Failed to execute SSH command to query Windows network drives")?;

    if !output.status.success() {
        bail!("Failed to query Windows mapped drives: {}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout_str.is_empty() {
        return Ok(vec![]);
    }

    let mut results = Vec::new();
    // PowerShell ConvertTo-Json returns either an array `[...]` or a single object `{...}`
    if stdout_str.starts_with('[') {
        let disks: Vec<WinLogicalDisk> = serde_json::from_str(&stdout_str)
            .context("Failed to parse Windows mapped drives JSON array")?;
        for d in disks {
            if let Some(prov) = d.provider_name {
                results.push((d.device_id, prov));
            }
        }
    } else if stdout_str.starts_with('{') {
        let disk: WinLogicalDisk = serde_json::from_str(&stdout_str)
            .context("Failed to parse Windows mapped drive JSON object")?;
        if let Some(prov) = disk.provider_name {
            results.push((disk.device_id, prov));
        }
    }

    Ok(results)
}

/// Synchronizes the mount table by correlating Samba shares with Windows mapped drives
pub fn sync_mount_table(config: &mut Config) -> Result<Vec<(String, String, String)>> {
    let conf_path_str = config.samba.config_path.as_deref().unwrap_or("/etc/samba/smb-win11.conf");
    let conf_path = Path::new(conf_path_str);

    let shares = parse_samba_shares(conf_path)
        .with_context(|| format!("Failed to parse Samba configuration from {:?}", conf_path))?;

    let user = &config.server.user;
    let host = crate::resolve_host_ip(config);
    let win_drives = query_windows_mapped_drives(user, &host)?;

    let mut synced = Vec::new();

    for (drive, provider) in win_drives {
        let clean_provider = provider.replace('/', "\\");
        let share_name = clean_provider.trim_end_matches('\\').split('\\').last().unwrap_or("");
        
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

    let drive = format!("{}:", &trimmed[..1].to_ascii_uppercase());
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

    println!("{:<18} {:<36} {}", "WINDOWS DRIVE", "LINUX HOST PATH", "STATUS");
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
