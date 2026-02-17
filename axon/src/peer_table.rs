use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock as StdRwLock, RwLockWriteGuard as StdRwLockWriteGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

use crate::config::{KnownPeer, StaticPeerConfig};
use crate::message::AgentId;

/// Sync-safe pubkey map shared with TLS verifiers.
///
/// Uses `std::sync::RwLock` (not `tokio::sync`) because rustls verifier
/// callbacks are synchronous. This map is the single source of truth for
/// peer public keys — updated automatically by `PeerTable` mutations.
pub type PubkeyMap = Arc<StdRwLock<HashMap<String, String>>>;

pub const STALE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerSource {
    Static,
    Discovered,
    Cached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Discovered,
    Connecting,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone)]
pub struct PeerRecord {
    pub agent_id: AgentId,
    pub addr: SocketAddr,
    pub pubkey: String,
    pub source: PeerSource,
    pub status: ConnectionStatus,
    pub rtt_ms: Option<f64>,
    pub last_seen: Instant,
}

impl PeerRecord {
    pub fn from_static(cfg: &StaticPeerConfig) -> Self {
        Self {
            agent_id: cfg.agent_id.clone(),
            addr: cfg.addr,
            pubkey: cfg.pubkey.clone(),
            source: PeerSource::Static,
            status: ConnectionStatus::Discovered,
            rtt_ms: None,
            last_seen: Instant::now(),
        }
    }

    pub fn from_cached(peer: &KnownPeer) -> Self {
        Self {
            agent_id: peer.agent_id.clone(),
            addr: peer.addr,
            pubkey: peer.pubkey.clone(),
            source: PeerSource::Cached,
            status: ConnectionStatus::Discovered,
            rtt_ms: None,
            last_seen: Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerTable {
    inner: Arc<RwLock<HashMap<AgentId, PeerRecord>>>,
    pubkeys: PubkeyMap,
}

impl Default for PeerTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerTable {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            pubkeys: Arc::new(StdRwLock::new(HashMap::new())),
        }
    }

    /// Returns the sync-safe pubkey map for sharing with TLS verifiers.
    pub fn pubkey_map(&self) -> PubkeyMap {
        self.pubkeys.clone()
    }

    fn pubkeys_write_guard(
        &self,
        operation: &'static str,
    ) -> StdRwLockWriteGuard<'_, HashMap<String, String>> {
        match self.pubkeys.write() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!(
                    operation = operation,
                    "pubkey map lock poisoned; recovering map to keep TLS pinset available"
                );
                poisoned.into_inner()
            }
        }
    }

    pub async fn upsert_discovered(&self, agent_id: AgentId, addr: SocketAddr, pubkey: String) {
        let mut table = self.inner.write().await;
        table
            .entry(agent_id.clone())
            .and_modify(|existing| {
                if existing.source != PeerSource::Static {
                    existing.addr = addr;
                    existing.pubkey = pubkey.clone();
                    existing.source = PeerSource::Discovered;
                }
                existing.last_seen = Instant::now();
            })
            .or_insert_with(|| PeerRecord {
                agent_id: agent_id.clone(),
                addr,
                pubkey: pubkey.clone(),
                source: PeerSource::Discovered,
                status: ConnectionStatus::Discovered,
                rtt_ms: None,
                last_seen: Instant::now(),
            });
        let mut map = self.pubkeys_write_guard("upsert_discovered");
        map.insert(agent_id.to_string(), pubkey);
    }

    pub async fn upsert_static(&self, cfg: &StaticPeerConfig) {
        let mut table = self.inner.write().await;
        table.insert(cfg.agent_id.clone(), PeerRecord::from_static(cfg));
        let mut map = self.pubkeys_write_guard("upsert_static");
        map.insert(cfg.agent_id.to_string(), cfg.pubkey.clone());
    }

    pub async fn upsert_cached(&self, peer: &KnownPeer) {
        let mut table = self.inner.write().await;
        let inserted = table
            .entry(peer.agent_id.clone())
            .or_insert_with(|| PeerRecord::from_cached(peer));
        let mut map = self.pubkeys_write_guard("upsert_cached");
        map.entry(peer.agent_id.to_string())
            .or_insert_with(|| inserted.pubkey.clone());
    }

    pub async fn remove(&self, agent_id: &str) -> Option<PeerRecord> {
        let mut table = self.inner.write().await;
        let removed = table.remove(agent_id);
        if removed.is_some() {
            let mut map = self.pubkeys_write_guard("remove");
            map.remove(agent_id);
        }
        removed
    }

    pub async fn get(&self, agent_id: &str) -> Option<PeerRecord> {
        let table = self.inner.read().await;
        table.get(agent_id).cloned()
    }

    pub async fn list(&self) -> Vec<PeerRecord> {
        let table = self.inner.read().await;
        let mut peers: Vec<PeerRecord> = table.values().cloned().collect();
        peers.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        peers
    }

    pub async fn set_status(&self, agent_id: &str, status: ConnectionStatus) {
        let mut table = self.inner.write().await;
        if let Some(peer) = table.get_mut(agent_id) {
            peer.status = status;
        }
    }

    pub async fn set_connected(&self, agent_id: &str, rtt_ms: Option<f64>) {
        let mut table = self.inner.write().await;
        if let Some(peer) = table.get_mut(agent_id) {
            peer.status = ConnectionStatus::Connected;
            peer.rtt_ms = rtt_ms;
        }
    }

    pub async fn set_disconnected(&self, agent_id: &str) {
        let mut table = self.inner.write().await;
        if let Some(peer) = table.get_mut(agent_id) {
            peer.status = ConnectionStatus::Disconnected;
            peer.rtt_ms = None;
        }
    }

    pub async fn set_rtt(&self, agent_id: &str, rtt_ms: f64) {
        let mut table = self.inner.write().await;
        if let Some(peer) = table.get_mut(agent_id) {
            peer.rtt_ms = Some(rtt_ms);
        }
    }

    pub async fn touch(&self, agent_id: &str) {
        let mut table = self.inner.write().await;
        if let Some(peer) = table.get_mut(agent_id) {
            peer.last_seen = Instant::now();
        }
    }

    pub async fn remove_stale(&self, ttl: Duration) -> Vec<AgentId> {
        let mut table = self.inner.write().await;
        let now = Instant::now();
        let stale: Vec<AgentId> = table
            .values()
            .filter(|p| p.source == PeerSource::Discovered && now.duration_since(p.last_seen) > ttl)
            .map(|p| p.agent_id.clone())
            .collect();
        for id in &stale {
            table.remove(id);
        }
        if !stale.is_empty() {
            let mut map = self.pubkeys_write_guard("remove_stale");
            for id in &stale {
                map.remove(id.as_str());
            }
        }
        stale
    }

    pub async fn peers_needing_connection(&self) -> Vec<PeerRecord> {
        let table = self.inner.read().await;
        table
            .values()
            .filter(|p| p.status == ConnectionStatus::Discovered)
            .cloned()
            .collect()
    }

    pub async fn to_known_peers(&self) -> Vec<KnownPeer> {
        let table = self.inner.read().await;
        table
            .values()
            .map(|peer| KnownPeer {
                agent_id: peer.agent_id.clone(),
                addr: peer.addr,
                pubkey: peer.pubkey.clone(),
                last_seen_unix_ms: crate::message::now_millis(),
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "peer_table_tests.rs"]
mod tests;
