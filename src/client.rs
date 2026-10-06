use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::RemoteAppInfo;

const DEFAULT_AGENT_PORT: u16 = 49152;
const TCP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Serialize, Debug)]
#[serde(tag = "action")]
enum AgentRequest<'a> {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "run")]
    Run {
        target: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        params: Option<&'a str>,
    },
    #[serde(rename = "open")]
    Open { file: &'a str },
    #[serde(rename = "list_apps")]
    ListApps { with_icons: bool },
    #[serde(rename = "get_mapped_drives")]
    GetMappedDrives,
    #[serde(rename = "quit")]
    Quit,
}

#[derive(Deserialize, Debug)]
struct AgentResponse {
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    apps: Option<Vec<RemoteAppInfo>>,
    #[serde(default)]
    active_ssh_count: Option<usize>,
    #[serde(default)]
    drives: Option<Vec<(String, String)>>,
}

pub struct AgentClient {
    pub host: String,
    pub port: u16,
}

impl AgentClient {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            port: if port == 0 { DEFAULT_AGENT_PORT } else { port },
        }
    }

    fn connect(&self, read_timeout: Duration) -> Result<TcpStream> {
        self.connect_with_timeout(TCP_TIMEOUT, read_timeout)
    }

    fn connect_with_timeout(&self, conn_timeout: Duration, read_timeout: Duration) -> Result<TcpStream> {
        let addr_str = format!("{}:{}", self.host, self.port);
        let addrs: Vec<SocketAddr> = std::net::ToSocketAddrs::to_socket_addrs(&addr_str)
            .with_context(|| format!("Failed to resolve agent address: {}", addr_str))?
            .collect();

        for addr in addrs {
            if let Ok(stream) = TcpStream::connect_timeout(&addr, conn_timeout) {
                let _ = stream.set_read_timeout(Some(read_timeout));
                let _ = stream.set_write_timeout(Some(conn_timeout));
                return Ok(stream);
            }
        }

        bail!("Unable to connect to Windows agent at {}:{}", self.host, self.port);
    }

    fn send_request(&self, req: &AgentRequest, read_timeout: Duration) -> Result<AgentResponse> {
        let mut stream = self.connect(read_timeout)?;
        let mut json_req = serde_json::to_string(req)?;
        json_req.push('\n');

        stream.write_all(json_req.as_bytes())
            .context("Failed to write request to agent TCP socket")?;
        stream.flush().context("Failed to flush TCP stream")?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)
            .context("Failed to read response from agent TCP socket")?;

        let resp: AgentResponse = serde_json::from_str(line.trim())
            .with_context(|| format!("Failed to parse agent JSON response: '{}'", line.trim()))?;

        if resp.status != "ok" {
            let msg = resp.message.unwrap_or_else(|| "Unknown agent error".to_string());
            bail!("Agent returned error: {}", msg);
        }

        Ok(resp)
    }

    pub fn ping(&self) -> Result<()> {
        self.ping_timeout(Duration::from_millis(200))
    }

    pub fn ping_timeout(&self, timeout: Duration) -> Result<()> {
        let mut stream = self.connect_with_timeout(timeout, timeout)?;
        let mut json_req = serde_json::to_string(&AgentRequest::Ping)?;
        json_req.push('\n');

        stream.write_all(json_req.as_bytes())
            .context("Failed to write ping to agent TCP socket")?;
        stream.flush().context("Failed to flush TCP stream")?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)
            .context("Failed to read ping response from agent TCP socket")?;

        let resp: AgentResponse = serde_json::from_str(line.trim())
            .with_context(|| format!("Failed to parse agent JSON response: '{}'", line.trim()))?;

        if resp.status == "ok" {
            Ok(())
        } else {
            bail!("Agent returned non-ok ping status")
        }
    }

    pub fn get_active_ssh_count(&self) -> Result<usize> {
        let resp = self.send_request(&AgentRequest::Status, Duration::from_secs(2))?;
        Ok(resp.active_ssh_count.unwrap_or(0))
    }

    pub fn get_mapped_drives(&self) -> Result<Vec<(String, String)>> {
        let resp = self.send_request(&AgentRequest::GetMappedDrives, Duration::from_secs(5))?;
        Ok(resp.drives.unwrap_or_default())
    }

    pub fn open_file(&self, win_path: &str) -> Result<()> {
        let _ = self.send_request(&AgentRequest::Open { file: win_path }, Duration::from_secs(5))?;
        Ok(())
    }

    pub fn run_target(&self, target: &str, params: Option<&str>) -> Result<()> {
        let _ = self.send_request(&AgentRequest::Run { target, params }, Duration::from_secs(5))?;
        Ok(())
    }

    pub fn list_apps(&self, with_icons: bool) -> Result<Vec<RemoteAppInfo>> {
        let resp = self.send_request(&AgentRequest::ListApps { with_icons }, Duration::from_secs(60))?;
        Ok(resp.apps.unwrap_or_default())
    }

    pub fn stop_daemon(&self) -> Result<()> {
        let _ = self.send_request(&AgentRequest::Quit, Duration::from_secs(2));
        Ok(())
    }
}

/// Ensures the VM is running, FreeRDP session is active, and the TCP agent is listening
pub fn get_or_ensure_client(config: &Config) -> Result<AgentClient> {
    let host = crate::resolve_host_ip(config);
    let port = config.server.agent_port;
    let client = AgentClient::new(&host, port);

    // 1. Fast path: check if agent is already listening and responsive
    if client.ping().is_ok() {
        return Ok(client);
    }

    // 2. Slow path: Agent not responding, ensure FreeRDP session and VM are awake
    log::info!("[rdp-launcher] Agent at {}:{} is not reachable, ensuring RDP session...", host, client.port);
    crate::ensure_freerdp_session(config)?;

    // 3. Wait up to 10 seconds for the TCP listener to come online
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(500));
        if client.ping().is_ok() {
            log::info!("[rdp-launcher] TCP agent at {}:{} connected successfully!", host, client.port);
            return Ok(client);
        }
    }

    bail!("Timed out waiting for Windows agent TCP service on {}:{}", host, client.port);
}
