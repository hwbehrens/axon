use std::env;
use std::fs;
use std::net::SocketAddr;
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
    pub known_peers: PathBuf,
    pub replay_cache: PathBuf,
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
            replay_cache: root.join("replay_cache.json"),
            socket: root.join("axon.sock"),
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
pub struct Config {
    #[serde(default)]
    pub port: Option<u16>,
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
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StaticPeerConfig {
    pub agent_id: String,
    pub addr: SocketAddr,
    pub pubkey: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct KnownPeer {
    pub agent_id: String,
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

pub fn save_known_peers(path: &Path, peers: &[KnownPeer]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }

    let data = serde_json::to_vec_pretty(peers).context("failed to encode known peers")?;
    fs::write(path, data)
        .with_context(|| format!("failed to write known peers: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn config_defaults_when_missing() {
        let dir = tempdir().expect("temp dir");
        let cfg = Config::load(&dir.path().join("missing.toml")).expect("load missing config");
        assert_eq!(cfg.effective_port(None), 7100);
        assert!(cfg.peers.is_empty());
    }

    #[test]
    fn config_parses_static_peers() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
                port = 8111
                [[peers]]
                agent_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                addr = "127.0.0.1:7100"
                pubkey = "Zm9v"
            "#,
        )
        .expect("write config");

        let cfg = Config::load(&path).expect("load config");
        assert_eq!(cfg.effective_port(None), 8111);
        assert_eq!(cfg.peers.len(), 1);
        assert_eq!(cfg.peers[0].addr.to_string(), "127.0.0.1:7100");
    }

    #[test]
    fn cli_override_takes_precedence() {
        let cfg = Config {
            port: Some(8000),
            peers: Vec::new(),
        };
        assert_eq!(cfg.effective_port(Some(9999)), 9999);
        assert_eq!(cfg.effective_port(None), 8000);
    }

    #[test]
    fn invalid_toml_returns_error() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "{{{{not toml!").expect("write");
        assert!(Config::load(&path).is_err());
    }

    #[test]
    fn known_peers_roundtrip() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("known.json");
        let peers = vec![KnownPeer {
            agent_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            addr: "127.0.0.1:7100".parse().expect("addr"),
            pubkey: "Zm9v".to_string(),
            last_seen_unix_ms: 123,
        }];

        save_known_peers(&path, &peers).expect("save");
        let loaded = load_known_peers(&path).expect("load");
        assert_eq!(loaded, peers);
    }

    #[test]
    fn known_peers_empty_when_missing() {
        let dir = tempdir().expect("temp dir");
        let loaded = load_known_peers(&dir.path().join("missing.json")).expect("load");
        assert!(loaded.is_empty());
    }

    #[test]
    fn discover_paths_from_root() {
        let root = PathBuf::from("/tmp/axon-test");
        let paths = AxonPaths::from_root(root.clone());
        assert_eq!(paths.identity_key, root.join("identity.key"));
        assert_eq!(paths.identity_pub, root.join("identity.pub"));
        assert_eq!(paths.config, root.join("config.toml"));
        assert_eq!(paths.known_peers, root.join("known_peers.json"));
        assert_eq!(paths.replay_cache, root.join("replay_cache.json"));
        assert_eq!(paths.socket, root.join("axon.sock"));
    }

    #[test]
    fn ensure_root_creates_and_sets_perms() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path().join("axon-subdir");
        let paths = AxonPaths::from_root(root.clone());
        paths.ensure_root_exists().expect("ensure root");
        assert!(root.exists());
        let mode = fs::metadata(&root).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
