use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use std::time::Instant;

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

    pub async fn list_peers(&self) -> Vec<Peer> {
        let peers = self.peers.read().await;
        peers.values().cloned().collect()
    }
}
