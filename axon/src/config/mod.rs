use std::env;
use std::fs;
use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tracing::warn;

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
    pub fn discover_with_override(override_root: Option<&Path>) -> Result<Self> {
        if let Some(root) = override_root {
            return Ok(Self::from_root(root.to_path_buf()));
        }

        if let Ok(root) = env::var("AXON_ROOT")
            && !root.trim().is_empty()
        {
            return Ok(Self::from_root(PathBuf::from(root)));
        }

        Self::discover()
    }

    pub fn discover() -> Result<Self> {
        let home = env::var("HOME").context("HOME is not set")?;
        let root = Path::new(&home).join(".axon");
        Ok(Self::from_root(root))
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self {
            identity_key: root.join("identity.key"),
            identity_pub: root.join("identity.pub"),
            config: root.join("config.yaml"),
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
    pub advertise_addr: Option<String>,
    #[serde(default)]
    pub peers: Vec<StaticPeerConfig>,
}

impl Config {
    pub async fn load(path: &Path) -> Result<Self> {
        let persisted = load_persisted_config(path).await?;
        Ok(persisted.resolve(path).await)
    }

    pub fn effective_port(&self, cli_override: Option<u16>) -> u16 {
        cli_override.or(self.port).unwrap_or(7100)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerAddr {
    Socket(SocketAddr),
    Host { host: String, port: u16 },
}

impl PeerAddr {
    fn resolve_host(host: &str, port: u16) -> Result<SocketAddr> {
        let addrs: Vec<SocketAddr> = (host, port)
            .to_socket_addrs()
            .with_context(|| format!("failed to resolve '{host}:{port}'"))?
            .collect();
        if let Some(addr) = addrs.iter().copied().find(SocketAddr::is_ipv4) {
            return Ok(addr);
        }
        addrs
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("resolution returned no addresses for '{host}:{port}'"))
    }

    pub fn parse(input: &str) -> Result<Self> {
        if let Ok(addr) = input.parse::<SocketAddr>() {
            return Ok(Self::Socket(addr));
        }

        let (host, port) = input
            .rsplit_once(':')
            .ok_or_else(|| anyhow!("expected host:port or ip:port"))?;
        if host.is_empty() {
            anyhow::bail!("host cannot be empty");
        }

        let port = port
            .parse::<u16>()
            .with_context(|| format!("invalid port '{port}'"))?;
        Ok(Self::Host {
            host: host.to_string(),
            port,
        })
    }

    pub fn resolve(&self) -> Result<SocketAddr> {
        match self {
            PeerAddr::Socket(addr) => Ok(*addr),
            PeerAddr::Host { host, port } => Self::resolve_host(host, *port),
        }
    }

    pub async fn resolve_for_config_load(&self) -> Result<SocketAddr> {
        match self {
            PeerAddr::Socket(addr) => Ok(*addr),
            PeerAddr::Host { host, port } => {
                let host_for_lookup = host.clone();
                let host_for_error = host.clone();
                let port = *port;
                tokio::task::spawn_blocking(move || Self::resolve_host(&host_for_lookup, port))
                    .await
                    .map_err(|err| {
                        anyhow!(
                            "hostname resolution task failed for '{host_for_error}:{port}': {err}"
                        )
                    })?
            }
        }
    }
}

impl std::fmt::Display for PeerAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerAddr::Socket(addr) => write!(f, "{addr}"),
            PeerAddr::Host { host, port } => write!(f, "{host}:{port}"),
        }
    }
}

impl<'de> Deserialize<'de> for PeerAddr {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl Serialize for PeerAddr {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self {
            PeerAddr::Socket(addr) => serializer.serialize_str(&addr.to_string()),
            PeerAddr::Host { host, port } => serializer.serialize_str(&format!("{host}:{port}")),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StaticPeerConfig {
    pub agent_id: AgentId,
    pub addr: SocketAddr,
    pub pubkey: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PersistedConfig {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advertise_addr: Option<String>,
    #[serde(default)]
    pub peers: Vec<PersistedStaticPeerConfig>,
}

impl PersistedConfig {
    async fn resolve(self, path: &Path) -> Config {
        let mut peers = Vec::with_capacity(self.peers.len());
        for peer in self.peers {
            match peer.addr.resolve_for_config_load().await {
                Ok(addr) => peers.push(StaticPeerConfig {
                    agent_id: peer.agent_id,
                    addr,
                    pubkey: peer.pubkey,
                }),
                Err(err) => {
                    warn!(
                        agent_id = %peer.agent_id,
                        addr = %peer.addr,
                        error = %err,
                        config = %path.display(),
                        "skipping static peer with invalid or unresolved addr"
                    );
                }
            }
        }

        Config {
            name: self.name,
            port: self.port,
            advertise_addr: self.advertise_addr,
            peers,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PersistedStaticPeerConfig {
    pub agent_id: AgentId,
    pub addr: PeerAddr,
    pub pubkey: String,
}

pub async fn load_persisted_config(path: &Path) -> Result<PersistedConfig> {
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(PersistedConfig::default()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read config: {}", path.display()));
        }
    };
    let parsed = serde_yaml::from_str::<PersistedConfig>(&raw)
        .with_context(|| format!("failed to parse config: {}", path.display()))?;
    Ok(parsed)
}

pub async fn save_persisted_config(path: &Path, config: &PersistedConfig) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }

    let serialized = serde_yaml::to_string(config)
        .with_context(|| format!("failed to serialize config: {}", path.display()))?;
    tokio::fs::write(path, serialized)
        .await
        .with_context(|| format!("failed to write config: {}", path.display()))?;
    Ok(())
}

pub async fn append_static_peer(path: &Path, peer: PersistedStaticPeerConfig) -> Result<()> {
    let mut config = load_persisted_config(path).await?;
    config.peers.push(peer);
    save_persisted_config(path, &config).await
}

pub async fn resolve_static_peer(
    agent_id: AgentId,
    addr: &str,
    pubkey: String,
) -> Result<StaticPeerConfig> {
    let addr = PeerAddr::parse(addr)?;
    let resolved = addr.resolve_for_config_load().await?;
    Ok(StaticPeerConfig {
        agent_id,
        addr: resolved,
        pubkey,
    })
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
#[path = "tests.rs"]
mod tests;
