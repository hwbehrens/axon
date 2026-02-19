use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock as StdRwLock, RwLockWriteGuard as StdRwLockWriteGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::config::{KnownPeer, StaticPeerConfig};
use crate::message::AgentId;

/// Sync-safe pubkey map shared with TLS verifiers.
///
/// Uses `std::sync::RwLock` (not `tokio::sync`) because rustls verifier
/// callbacks are synchronous. This map is the single source of truth for
/// peer public keys — updated automatically by `PeerTable` mutations.
pub type PubkeyMap = Arc<StdRwLock<HashMap<String, String>>>;

pub const STALE_TIMEOUT: Duration = Duration::from_secs(60);

fn canonical_agent_id(agent_id: &str) -> AgentId {
    AgentId::from(agent_id.to_ascii_lowercase())
}

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
        let agent_id = canonical_agent_id(cfg.agent_id.as_str());
        Self {
            agent_id,
            addr: cfg.addr,
            pubkey: cfg.pubkey.clone(),
            source: PeerSource::Static,
            status: ConnectionStatus::Discovered,
            rtt_ms: None,
            last_seen: Instant::now(),
        }
    }

    pub fn from_cached(peer: &KnownPeer) -> Self {
        let agent_id = canonical_agent_id(peer.agent_id.as_str());
        Self {
            agent_id,
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

    /// Evict all non-static peers at `addr` that have a different `agent_id`.
    /// Returns the evicted agent IDs.
    fn evict_stale_at_addr(
        table: &mut HashMap<AgentId, PeerRecord>,
        new_agent_id: &AgentId,
        addr: SocketAddr,
    ) -> Vec<AgentId> {
        let stale_ids: Vec<AgentId> = table
            .values()
            .filter(|p| {
                p.addr == addr && p.agent_id != *new_agent_id && p.source != PeerSource::Static
            })
            .map(|p| p.agent_id.clone())
            .collect();
        for id in &stale_ids {
            table.remove(id.as_str());
        }
        stale_ids
    }

    pub async fn upsert_discovered(&self, agent_id: AgentId, addr: SocketAddr, pubkey: String) {
        let agent_id = canonical_agent_id(agent_id.as_str());
        let mut table = self.inner.write().await;
        // O1: block insertion when a static peer already occupies the address
        let static_conflict = table
            .values()
            .any(|p| p.addr == addr && p.agent_id != agent_id && p.source == PeerSource::Static);
        if static_conflict {
            debug!(agent_id = agent_id.as_str(), %addr, "skipping discovered peer; static peer already occupies address");
            return;
        }
        let evicted = Self::evict_stale_at_addr(&mut table, &agent_id, addr);
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
        for id in &evicted {
            map.remove(id.as_str());
            info!(evicted = id.as_str(), new = agent_id.as_str(), %addr, "evicted stale peer at same address");
        }
        map.insert(agent_id.to_string(), pubkey);
    }

    pub async fn upsert_static(&self, cfg: &StaticPeerConfig) {
        let agent_id = canonical_agent_id(cfg.agent_id.as_str());
        let mut table = self.inner.write().await;
        // O1: static peers are authoritative; evict any peer at the same address
        let evicted: Vec<AgentId> = table
            .values()
            .filter(|p| p.addr == cfg.addr && p.agent_id != agent_id)
            .map(|p| p.agent_id.clone())
            .collect();
        for id in &evicted {
            table.remove(id.as_str());
        }
        table.insert(agent_id.clone(), PeerRecord::from_static(cfg));
        let mut map = self.pubkeys_write_guard("upsert_static");
        for id in &evicted {
            map.remove(id.as_str());
            info!(evicted = id.as_str(), new = agent_id.as_str(), addr = %cfg.addr, "evicted peer at same address (static peer override)");
        }
        map.insert(agent_id.to_string(), cfg.pubkey.clone());
    }

    pub async fn upsert_cached(&self, peer: &KnownPeer) {
        let agent_id = canonical_agent_id(peer.agent_id.as_str());
        let mut table = self.inner.write().await;
        // O1: block insertion when a static peer already occupies the address
        let static_conflict = table.values().any(|p| {
            p.addr == peer.addr && p.agent_id != agent_id && p.source == PeerSource::Static
        });
        if static_conflict {
            debug!(agent_id = agent_id.as_str(), addr = %peer.addr, "skipping cached peer; static peer already occupies address");
            return;
        }
        let evicted = Self::evict_stale_at_addr(&mut table, &agent_id, peer.addr);
        let inserted = table
            .entry(agent_id.clone())
            .or_insert_with(|| PeerRecord::from_cached(peer));
        let mut map = self.pubkeys_write_guard("upsert_cached");
        for id in &evicted {
            map.remove(id.as_str());
            info!(evicted = id.as_str(), new = agent_id.as_str(), addr = %peer.addr, "evicted stale peer at same address");
        }
        map.entry(agent_id.to_string())
            .or_insert_with(|| inserted.pubkey.clone());
    }

    pub async fn remove(&self, agent_id: &str) -> Option<PeerRecord> {
        let agent_id = canonical_agent_id(agent_id);
        let mut table = self.inner.write().await;
        let removed = table.remove(agent_id.as_str());
        if removed.is_some() {
            let mut map = self.pubkeys_write_guard("remove");
            map.remove(agent_id.as_str());
        }
        removed
    }

    pub async fn get(&self, agent_id: &str) -> Option<PeerRecord> {
        let agent_id = canonical_agent_id(agent_id);
        let table = self.inner.read().await;
        table.get(agent_id.as_str()).cloned()
    }

    pub async fn list(&self) -> Vec<PeerRecord> {
        let table = self.inner.read().await;
        let mut peers: Vec<PeerRecord> = table.values().cloned().collect();
        peers.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        peers
    }

    pub async fn set_status(&self, agent_id: &str, status: ConnectionStatus) {
        let agent_id = canonical_agent_id(agent_id);
        let mut table = self.inner.write().await;
        if let Some(peer) = table.get_mut(agent_id.as_str()) {
            peer.status = status;
        }
    }

    pub async fn set_connected(&self, agent_id: &str, rtt_ms: Option<f64>) {
        let agent_id = canonical_agent_id(agent_id);
        let mut table = self.inner.write().await;
        if let Some(peer) = table.get_mut(agent_id.as_str()) {
            peer.status = ConnectionStatus::Connected;
            peer.rtt_ms = rtt_ms;
        }
    }

    pub async fn set_disconnected(&self, agent_id: &str) {
        let agent_id = canonical_agent_id(agent_id);
        let mut table = self.inner.write().await;
        if let Some(peer) = table.get_mut(agent_id.as_str()) {
            peer.status = ConnectionStatus::Disconnected;
            peer.rtt_ms = None;
        }
    }

    pub async fn set_rtt(&self, agent_id: &str, rtt_ms: f64) {
        let agent_id = canonical_agent_id(agent_id);
        let mut table = self.inner.write().await;
        if let Some(peer) = table.get_mut(agent_id.as_str()) {
            peer.rtt_ms = Some(rtt_ms);
        }
    }

    pub async fn touch(&self, agent_id: &str) {
        let agent_id = canonical_agent_id(agent_id);
        let mut table = self.inner.write().await;
        if let Some(peer) = table.get_mut(agent_id.as_str()) {
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
#[path = "tests/mod.rs"]
mod tests;
