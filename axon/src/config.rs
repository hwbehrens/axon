use std::env;
use std::fs;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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
    pub socket: PathBuf,
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
            socket: root.join("axon.sock"),
            root,
        }
    }

    pub fn ensure_root_exists(&self) -> Result<()> {
        if self.root.exists() {
            // Reject symlinked root directory (security: IPC.md §2.2)
            let meta = fs::symlink_metadata(&self.root).with_context(|| {
                format!(
                    "failed to read metadata for AXON root: {}",
                    self.root.display()
                )
            })?;
            if meta.file_type().is_symlink() {
                anyhow::bail!(
                    "AXON root directory is a symlink (security violation): {}. \
                     Remove the symlink and restart.",
                    self.root.display()
                );
            }
        } else {
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
pub struct Config {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub peers: Vec<StaticPeerConfig>,
}

impl Config {
    pub async fn load(path: &Path) -> Result<Self> {
        let raw = match tokio::fs::read_to_string(path).await {
            Ok(raw) => raw,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read config: {}", path.display()));
            }
        };
        let parsed = toml::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse config: {}", path.display()))?;
        Ok(parsed)
    }

    pub fn effective_port(&self, cli_override: Option<u16>) -> u16 {
        cli_override.or(self.port).unwrap_or(7100)
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

pub async fn load_known_peers(path: &Path) -> Result<Vec<KnownPeer>> {
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read known peers: {}", path.display()));
        }
    };
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
