use std::env;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct AxonPaths {
    pub root: PathBuf,
    pub identity_key: PathBuf,
    pub identity_pub: PathBuf,
    pub config: PathBuf,
    pub peers: PathBuf,
    pub legacy_known_peers: PathBuf,
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

pub async fn load_persisted_config(path: &Path) -> Result<PersistedConfig> {
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(PersistedConfig::default()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read config: {}", path.display()));
        }
    };
    serde_yaml::from_str(&raw).with_context(|| {
        format!(
            "failed to parse config: {}. Legacy `peers` entries are unsupported; re-enroll peers with `axon connect`.",
            path.display()
        )
    })
}

pub async fn save_persisted_config(path: &Path, config: &PersistedConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create config directory: {}", parent.display()))?;
    }
    let serialized = serde_yaml::to_string(config)
        .with_context(|| format!("failed to serialize config: {}", path.display()))?;
    tokio::fs::write(path, serialized)
        .await
        .with_context(|| format!("failed to write config: {}", path.display()))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
