use std::process::Command;
use std::time::{Duration, Instant};
use anyhow::{Context, Result};
use crate::config::Config;
use crate::is_freerdp_running;

pub fn get_vm_state(vm_name: &str) -> Result<String> {
    let output = Command::new("virsh")
        .args(&["domstate", vm_name])
        .output()
        .context("Failed to execute virsh domstate")?;

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(text)
}

pub fn get_vm_ips(vm_name: &str) -> Vec<String> {
    let mut ips = Vec::new();

    // 1. Try qemu guest agent first (reads all interfaces from inside Windows)
    if let Ok(out) = Command::new("virsh").args(&["domifaddr", vm_name, "--source", "agent"]).output() {
        if out.status.success() {
            parse_domifaddr(&String::from_utf8_lossy(&out.stdout), &mut ips);
        }
    }

    // 2. Fallback to DHCP lease table if guest agent returned nothing
    if ips.is_empty() {
        if let Ok(out) = Command::new("virsh").args(&["domifaddr", vm_name]).output() {
            if out.status.success() {
                parse_domifaddr(&String::from_utf8_lossy(&out.stdout), &mut ips);
            }
        }
    }

    ips
}

fn parse_domifaddr(output: &str, ips: &mut Vec<String>) {
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        for p in parts {
            if p.contains('/') && !p.starts_with("127.") && !p.starts_with("::1") {
                let addr = p.split('/').next().unwrap_or(p);
                let clean_ip = addr.split('%').next().unwrap_or(addr);
                if !ips.contains(&clean_ip.to_string()) {
                    ips.push(clean_ip.to_string());
                }
            }
        }
    }
}

pub fn has_active_ssh(vm_ips: &[String], fallback_host: &str) -> bool {
    let mut targets: Vec<&str> = vm_ips.iter().map(|s| s.as_str()).collect();
    if targets.is_empty() {
        targets.push(fallback_host);
    }

    for ip in targets {
        // Check for established TCP connections on port 22
        let filter = format!("dport = :22 and dst {}", ip);
        if let Ok(out) = Command::new("ss").args(&["-Htn", &filter]).output() {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.lines().any(|l| l.contains("ESTAB")) {
                return true;
            }
        }
    }
    false
}

pub fn count_active_rdp_windows() -> (usize, Vec<String>) {
    let mut titles = Vec::new();
    let output = Command::new("niri").args(&["msg", "-j", "windows"]).output();

    if let Ok(out) = output {
        if out.status.success() {
            if let Ok(windows) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) {
                for w in windows {
                    if let Some(app_id) = w.get("app_id").and_then(|v| v.as_str()) {
                        if app_id == "com.freerdp.client.sdl3" {
                            let title = w.get("title").and_then(|v| v.as_str()).unwrap_or("Untitled").to_string();
                            titles.push(title);
                        }
                    }
                }
            }
        }
    }

    let count = titles.len();
    (count, titles)
}

pub fn resume_vm_if_needed(vm_name: &str) -> Result<()> {
    let state = get_vm_state(vm_name)?;
    let s = state.to_lowercase();
    if s.contains("暂停") || s.contains("paused") || s.contains("suspended") {
        println!("[lifecycle] VM '{}' is currently paused/suspended. Resuming...", vm_name);
        let status = Command::new("virsh")
            .args(&["resume", vm_name])
            .status()
            .context("Failed to resume VM via virsh")?;
        if !status.success() {
            anyhow::bail!("Failed to resume VM '{}'", vm_name);
        }
        std::thread::sleep(Duration::from_millis(600));
    } else if s.contains("关机") || s.contains("shut off") {
        println!("[lifecycle] VM '{}' is shut off. Starting...", vm_name);
        let status = Command::new("virsh")
            .args(&["start", vm_name])
            .status()
            .context("Failed to start VM via virsh")?;
        if !status.success() {
            anyhow::bail!("Failed to start VM '{}'", vm_name);
        }
        println!("[lifecycle] Waiting for VM guest to initialize network...");
        std::thread::sleep(Duration::from_secs(5));
    }
    Ok(())
}

pub fn reclaim_virtio_mem(vm_name: &str, alias: &str) -> Result<()> {
    println!("[lifecycle] Reclaiming virtio-mem device '{}' memory back to host...", alias);
    let status = Command::new("virsh")
        .args(&[
            "update-memory-device",
            vm_name,
            "--alias",
            alias,
            "--requested-size",
            "0",
            "--live",
        ])
        .status()
        .context("Failed to update virtio-mem requested-size")?;

    if !status.success() {
        eprintln!("[lifecycle] Warning: update-memory-device returned non-zero code");
    } else {
        println!("[lifecycle] virtio-mem dynamic memory reclaimed successfully.");
    }
    Ok(())
}

pub fn suspend_vm(vm_name: &str) -> Result<()> {
    println!("[lifecycle] Suspending VM '{}' to RAM...", vm_name);
    let status = Command::new("virsh")
        .args(&["suspend", vm_name])
        .status()
        .context("Failed to suspend VM via virsh")?;

    if !status.success() {
        anyhow::bail!("Failed to suspend VM '{}'", vm_name);
    }
    println!("[lifecycle] VM '{}' is now suspended (CPU 0%).", vm_name);
    Ok(())
}

pub fn run_watcher(config: Config) -> Result<()> {
    let vm_name = &config.lifecycle.vm_name;
    let rdp_timeout_secs = config.lifecycle.idle_disconnect_timeout;
    let vm_timeout_secs = config.lifecycle.vm_suspend_timeout;

    println!("[lifecycle] Starting lifecycle watcher for VM '{}'...", vm_name);
    println!("[lifecycle] Idle disconnect timeout: {}s, VM suspend timeout: {}s", rdp_timeout_secs, vm_timeout_secs);

    let mut rdp_idle_start: Option<Instant> = None;
    let mut vm_idle_start: Option<Instant> = None;
    let mut cached_ips: Vec<String> = Vec::new();
    let mut last_ip_fetch = Instant::now() - Duration::from_secs(100);

    loop {
        std::thread::sleep(Duration::from_secs(2));

        // 1. Check VM state
        let vm_state = match get_vm_state(vm_name) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let s_lower = vm_state.to_lowercase();
        let is_running = s_lower.contains("运行") || s_lower.contains("running");

        if !is_running {
            rdp_idle_start = None;
            vm_idle_start = None;
            cached_ips.clear();
            continue;
        }

        // 2. Refresh VM IPs periodically (every 30s) or if empty
        if cached_ips.is_empty() || last_ip_fetch.elapsed() > Duration::from_secs(30) {
            let fetched = get_vm_ips(vm_name);
            if !fetched.is_empty() {
                cached_ips = fetched;
                last_ip_fetch = Instant::now();
            }
        }
        let ssh_active = has_active_ssh(&cached_ips, &config.server.host);

        // 3. Check FreeRDP session & window count
        let freerdp_active = is_freerdp_running();
        let (window_count, _titles) = if freerdp_active {
            count_active_rdp_windows()
        } else {
            (0, vec![])
        };

        // If there are active application windows or active SSH session, reset VM suspend timer
        if window_count > 0 || ssh_active {
            vm_idle_start = None;
        }

        // --- Handle FreeRDP disconnect timeout ---
        if freerdp_active {
            if window_count > 0 {
                rdp_idle_start = None;
            } else {
                let start = rdp_idle_start.get_or_insert_with(Instant::now);
                if rdp_timeout_secs > 0 && start.elapsed() >= Duration::from_secs(rdp_timeout_secs) {
                    println!("[lifecycle] No RemoteApp windows open for {}s. Disconnecting FreeRDP session...", rdp_timeout_secs);
                    let _ = crate::stop_remote_daemon(&config);
                    rdp_idle_start = None;
                }
            }
        } else {
            rdp_idle_start = None;
        }

        // --- Handle VM suspend timeout ---
        // VM can only be suspended if FreeRDP is NOT connected and NO active SSH connections exist
        if !freerdp_active && !ssh_active {
            let start = vm_idle_start.get_or_insert_with(Instant::now);
            if vm_timeout_secs > 0 && start.elapsed() >= Duration::from_secs(vm_timeout_secs) {
                println!("[lifecycle] Inactivity timeout ({}s) reached. Preparing VM suspend...", vm_timeout_secs);
                if config.lifecycle.reclaim_virtio_mem {
                    let _ = reclaim_virtio_mem(vm_name, &config.lifecycle.virtio_mem_alias);
                }
                let _ = suspend_vm(vm_name);
                vm_idle_start = None;
            }
        } else {
            vm_idle_start = None;
        }
    }
}

pub fn show_lifecycle_status(config: &Config) -> Result<()> {
    let vm_name = &config.lifecycle.vm_name;
    println!("=== RDP & VM Lifecycle Status ===");

    let state = get_vm_state(vm_name).unwrap_or_else(|e| format!("Error: {}", e));
    println!("VM Name:             {}", vm_name);
    println!("VM State:            {}", state);

    let ips = get_vm_ips(vm_name);
    if ips.is_empty() {
        println!("VM IP Addresses:     None detected");
    } else {
        println!("VM IP Addresses:     {}", ips.join(", "));
    }

    let ssh_active = has_active_ssh(&ips, &config.server.host);
    println!("Active SSH Sessions: {}", if ssh_active { "YES (Will prevent VM suspend)" } else { "None" });

    let freerdp_active = is_freerdp_running();
    println!("FreeRDP Connection:  {}", if freerdp_active { "Connected" } else { "Disconnected" });

    if freerdp_active {
        let (count, titles) = count_active_rdp_windows();
        println!("RemoteApp Windows:   {} active", count);
        for t in titles {
            println!("  - {}", t);
        }
    }

    println!("\nConfigured Timeouts:");
    println!("  - Idle RDP Disconnect: {}s", config.lifecycle.idle_disconnect_timeout);
    println!("  - Idle VM Suspend:    {}s", config.lifecycle.vm_suspend_timeout);
    println!("  - Reclaim virtio-mem: {} (alias: {})", config.lifecycle.reclaim_virtio_mem, config.lifecycle.virtio_mem_alias);

    Ok(())
}
