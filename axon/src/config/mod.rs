use std::env;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct AxonPaths {
    pub root: PathBuf,
    pub identity_key: PathBuf,
    pub identity_pub: PathBuf,
    pub config: PathBuf,
    pub peers: PathBuf,
    pub legacy_known_peers: PathBuf,
    pub socket: PathBuf,
    /// Runtime single-instance lock file (`daemon.pid`).
    pub daemon_lock: PathBuf,
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
        Ok(Self::from_root(Path::new(&home).join(".axon")))
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self {
            identity_key: root.join("identity.key"),
            identity_pub: root.join("identity.pub"),
            config: root.join("config.yaml"),
            peers: root.join("peers.json"),
            legacy_known_peers: root.join("known_peers.json"),
            socket: root.join("axon.sock"),
            daemon_lock: root.join("daemon.pid"),
            root,
        }
    }

    pub fn ensure_root_exists(&self) -> Result<()> {
        if self.root.exists() {
            let metadata = fs::symlink_metadata(&self.root)
                .with_context(|| format!("failed to inspect AXON root: {}", self.root.display()))?;
            if metadata.file_type().is_symlink() {
                anyhow::bail!(
                    "AXON root directory is a symlink: {}. Remove it and restart.",
                    self.root.display()
                );
            }
            // The state root holds identity and trust material, so an existing
            // (including user-supplied) root is narrowed to owner-only mode.
            // Surface the mutation instead of silently changing a directory
            // the operator created.
            if metadata.permissions().mode() & 0o777 != 0o700 {
                warn!(
                    root = %self.root.display(),
                    "normalizing existing AXON root permissions to 0700"
                );
            }
        } else {
            fs::create_dir_all(&self.root)
                .with_context(|| format!("failed to create AXON root: {}", self.root.display()))?;
        }
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to secure AXON root: {}", self.root.display()))?;
        Ok(())
    }

    pub fn reject_legacy_peer_state(&self) -> Result<()> {
        if self.legacy_known_peers.exists() {
            anyhow::bail!(
                "unsupported legacy peer state at {}. Remove known_peers.json and intentionally re-enroll peers with `axon connect`.",
                self.legacy_known_peers.display()
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub name: Option<String>,
    pub port: Option<u16>,
    pub advertise_addr: Option<String>,
}

impl Config {
    pub async fn load(path: &Path) -> Result<Self> {
        let persisted = load_persisted_config(path).await?;
        persisted
            .validate()
            .with_context(|| format!("invalid config: {}", path.display()))?;
        Ok(Self {
            name: persisted.name,
            port: persisted.port,
            advertise_addr: persisted.advertise_addr,
        })
    }

    pub fn effective_port(&self, cli_override: Option<u16>) -> u16 {
        cli_override.or(self.port).unwrap_or(7100)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PersistedConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advertise_addr: Option<String>,
}

impl PersistedConfig {
    /// Semantic validation for values that deserialize successfully but would
    /// break daemon consumers (port 0 binds an ephemeral endpoint, an empty
    /// display name reaches mDNS/IPC views).
    pub fn validate(&self) -> Result<()> {
        if let Some(name) = &self.name
            && name.trim().is_empty()
        {
            anyhow::bail!("name must not be empty");
        }
        if self.port == Some(0) {
            anyhow::bail!(
                "port 0 is not valid; QUIC requires a non-zero port for peer connections"
            );
        }
        Ok(())
    }
}

pub async fn load_persisted_config(path: &Path) -> Result<PersistedConfig> {
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(PersistedConfig::default()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read config: {}", path.display()));
        }
    };
    serde_saphyr::from_str(&raw).map_err(|err| {
        let message = format!("failed to parse config: {}: {err}", path.display());
        // Only mention the legacy `peers` migration path when that is actually
        // what failed; generic YAML errors must not carry misleading guidance.
        if err.to_string().contains("unknown field `peers`") {
            anyhow::anyhow!(
                "{message}. Legacy `peers` entries are unsupported; re-enroll peers with `axon connect`."
            )
        } else {
            anyhow::anyhow!(message)
        }
    })
}

pub async fn save_persisted_config(path: &Path, config: &PersistedConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create config directory: {}", parent.display()))?;
    }
    let serialized = serde_saphyr::to_string(config)
        .with_context(|| format!("failed to serialize config: {}", path.display()))?;

    // Temp-file + rename: a crash between write and replace must never leave a
    // truncated config.yaml, which the daemon would refuse on next startup.
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.yaml");
    let temp_path = path.with_file_name(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let result = async {
        tokio::fs::write(&temp_path, serialized)
            .await
            .with_context(|| format!("failed to write config: {}", temp_path.display()))?;
        tokio::fs::rename(&temp_path, path).await.with_context(|| {
            format!(
                "failed to atomically replace config {} with {}",
                path.display(),
                temp_path.display()
            )
        })
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temp_path).await;
    }
    result
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
