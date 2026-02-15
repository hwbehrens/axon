use std::env;
use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::message::AgentId;

#[derive(Debug, Clone)]
pub struct AxonPaths {
    pub root: PathBuf,
    pub identity_key: PathBuf,
    pub identity_pub: PathBuf,
    pub config: PathBuf,
    pub known_peers: PathBuf,
    pub replay_cache: PathBuf,
    pub socket: PathBuf,
    pub ipc_token: PathBuf,
}

impl AxonPaths {
    pub fn discover() -> Result<Self> {
        let home = env::var("HOME").context("HOME is not set")?;
        let root = Path::new(&home).join(".axon");
        Ok(Self::from_root(root))
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self {
            identity_key: root.join("identity.key"),
            identity_pub: root.join("identity.pub"),
            config: root.join("config.toml"),
            known_peers: root.join("known_peers.json"),
            replay_cache: root.join("replay_cache.json"),
            socket: root.join("axon.sock"),
            ipc_token: root.join("ipc-token"),
            root,
        }
    }

    pub fn ensure_root_exists(&self) -> Result<()> {
        if !self.root.exists() {
            fs::create_dir_all(&self.root).with_context(|| {
                format!("failed to create AXON root dir: {}", self.root.display())
            })?;
        }
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700)).with_context(|| {
            format!(
                "failed to set AXON dir permissions: {}",
                self.root.display()
            )
        })?;
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct IpcConfig {
    #[serde(default)]
    pub allow_v1: Option<bool>,
    #[serde(default)]
    pub buffer_size: Option<usize>,
    #[serde(default)]
    pub buffer_ttl_secs: Option<u64>,
    #[serde(default)]
    pub buffer_byte_cap: Option<usize>,
    #[serde(default)]
    pub token_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub max_ipc_clients: Option<usize>,
    #[serde(default)]
    pub max_connections: Option<usize>,
    #[serde(default)]
    pub keepalive_secs: Option<u64>,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub reconnect_max_backoff_secs: Option<u64>,
    #[serde(default)]
    pub handshake_timeout_secs: Option<u64>,
    #[serde(default)]
    pub inbound_read_timeout_secs: Option<u64>,
    #[serde(default)]
    pub ipc: Option<IpcConfig>,
    #[serde(default)]
    pub peers: Vec<StaticPeerConfig>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;
        let parsed = toml::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse config: {}", path.display()))?;
        Ok(parsed)
    }

    pub fn effective_port(&self, cli_override: Option<u16>) -> u16 {
        cli_override.or(self.port).unwrap_or(7100)
    }

    pub fn effective_max_ipc_clients(&self) -> usize {
        self.max_ipc_clients.unwrap_or(64)
    }

    pub fn effective_max_connections(&self) -> usize {
        self.max_connections.unwrap_or(128)
    }

    pub fn effective_keepalive(&self) -> Duration {
        Duration::from_secs(self.keepalive_secs.unwrap_or(15))
    }

    pub fn effective_idle_timeout(&self) -> Duration {
        Duration::from_secs(self.idle_timeout_secs.unwrap_or(60))
    }

    pub fn effective_reconnect_max_backoff(&self) -> Duration {
        Duration::from_secs(self.reconnect_max_backoff_secs.unwrap_or(30))
    }

    pub fn effective_handshake_timeout(&self) -> Duration {
        Duration::from_secs(self.handshake_timeout_secs.unwrap_or(5))
    }

    pub fn effective_inbound_read_timeout(&self) -> Duration {
        Duration::from_secs(self.inbound_read_timeout_secs.unwrap_or(10))
    }

    pub fn effective_allow_v1(&self) -> bool {
        self.ipc.as_ref().and_then(|c| c.allow_v1).unwrap_or(true)
    }

    pub fn effective_token_path(&self, root: &Path) -> PathBuf {
        self.ipc
            .as_ref()
            .and_then(|c| c.token_path.as_ref())
            .map(|p| {
                if let Some(rest) = p.strip_prefix("~/") {
                    if let Ok(home) = std::env::var("HOME") {
                        PathBuf::from(home).join(rest)
                    } else {
                        PathBuf::from(p)
                    }
                } else {
                    PathBuf::from(p)
                }
            })
            .unwrap_or_else(|| root.join("ipc-token"))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StaticPeerConfig {
    pub agent_id: AgentId,
    pub addr: SocketAddr,
    pub pubkey: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct KnownPeer {
    pub agent_id: AgentId,
    pub addr: SocketAddr,
    pub pubkey: String,
    pub last_seen_unix_ms: u64,
}

pub fn load_known_peers(path: &Path) -> Result<Vec<KnownPeer>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read known peers: {}", path.display()))?;
    let peers = serde_json::from_str::<Vec<KnownPeer>>(&raw)
        .with_context(|| format!("failed to parse known peers: {}", path.display()))?;
    Ok(peers)
}

pub async fn save_known_peers(path: &Path, peers: &[KnownPeer]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }

    let data = serde_json::to_vec(peers).context("failed to encode known peers")?;
    tokio::fs::write(path, data)
        .await
        .with_context(|| format!("failed to write known peers: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
