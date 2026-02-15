use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use std::time::Instant;
use anyhow::Result;
use tokio::fs;
use std::path::PathBuf;
use base64::Engine;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PeerStatus {
    Discovered,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone)]
pub struct Peer {
    pub agent_id: String,
    pub addr: SocketAddr,
    pub pubkey: Vec<u8>,
    pub status: PeerStatus,
    pub last_seen: Instant,
}

#[derive(Serialize, Deserialize)]
struct KnownPeer {
    agent_id: String,
    addr: SocketAddr,
    pubkey_base64: String,
}

pub struct PeerTable {
    peers: RwLock<HashMap<String, Peer>>,
}

impl PeerTable {
    pub fn new() -> Self {
        Self {
            peers: RwLock::new(HashMap::new()),
        }
    }

    pub async fn update_peer(&self, agent_id: String, addr: SocketAddr, pubkey: Vec<u8>) {
        let mut peers = self.peers.write().await;
        peers.insert(agent_id.clone(), Peer {
            agent_id,
            addr,
            pubkey,
            status: PeerStatus::Discovered,
            last_seen: Instant::now(),
        });
    }

    pub async fn set_status(&self, agent_id: &str, status: PeerStatus) {
        let mut peers = self.peers.write().await;
        if let Some(peer) = peers.get_mut(agent_id) {
            peer.status = status;
        }
    }

    pub async fn list_peers(&self) -> Vec<Peer> {
        let peers = self.peers.read().await;
        peers.values().cloned().collect()
    }

    pub async fn load_known_peers(&self, base_dir: &PathBuf) -> Result<()> {
        let path = base_dir.join(".axon/known_peers.json");
        if path.exists() {
            let content = fs::read_to_string(path).await?;
            let known: Vec<KnownPeer> = serde_json::from_str(&content)?;
            let mut peers = self.peers.write().await;
            for kp in known {
                if let Ok(pk) = base64::engine::general_purpose::STANDARD.decode(&kp.pubkey_base64) {
                    peers.insert(kp.agent_id.clone(), Peer {
                        agent_id: kp.agent_id,
                        addr: kp.addr,
                        pubkey: pk,
                        status: PeerStatus::Disconnected,
                        last_seen: Instant::now(),
                    });
                }
            }
        }
        Ok(())
    }

    pub async fn save_known_peers(&self, base_dir: &PathBuf) -> Result<()> {
        let peers = self.peers.read().await;
        let known: Vec<KnownPeer> = peers.values().map(|p| KnownPeer {
            agent_id: p.agent_id.clone(),
            addr: p.addr,
            pubkey_base64: base64::engine::general_purpose::STANDARD.encode(&p.pubkey),
        }).collect();
        
        let path = base_dir.join(".axon/known_peers.json");
        let content = serde_json::to_string_pretty(&known)?;
        fs::write(path, content).await?;
        Ok(())
    }
}
