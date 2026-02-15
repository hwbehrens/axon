use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use anyhow::Result;
use tokio::fs;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub peers: Vec<StaticPeer>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StaticPeer {
    pub agent_id: String,
    pub addr: SocketAddr,
    pub pubkey: String, // base64
}

fn default_port() -> u16 {
    7100
}

impl Config {
    pub async fn load(base_dir: &PathBuf) -> Result<Self> {
        let config_path = base_dir.join(".axon/config.toml");
        if config_path.exists() {
            let content = fs::read_to_string(config_path).await?;
            Ok(toml::from_str(&content)?)
        } else {
            Ok(Config {
                port: default_port(),
                peers: Vec::new(),
            })
        }
    }
}
