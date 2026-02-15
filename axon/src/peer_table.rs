use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::{KnownPeer, StaticPeerConfig};

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
    pub agent_id: String,
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
    inner: Arc<RwLock<HashMap<String, PeerRecord>>>,
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
        }
    }

    pub async fn upsert_discovered(
        &self,
        agent_id: String,
        addr: SocketAddr,
        pubkey: String,
    ) {
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
                agent_id,
                addr,
                pubkey,
                source: PeerSource::Discovered,
                status: ConnectionStatus::Discovered,
                rtt_ms: None,
                last_seen: Instant::now(),
            });
    }

    pub async fn upsert_static(&self, cfg: &StaticPeerConfig) {
        let mut table = self.inner.write().await;
        table.insert(cfg.agent_id.clone(), PeerRecord::from_static(cfg));
    }

    pub async fn upsert_cached(&self, peer: &KnownPeer) {
        let mut table = self.inner.write().await;
        table
            .entry(peer.agent_id.clone())
            .or_insert_with(|| PeerRecord::from_cached(peer));
    }

    pub async fn remove(&self, agent_id: &str) -> Option<PeerRecord> {
        let mut table = self.inner.write().await;
        table.remove(agent_id)
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

    pub async fn remove_stale(&self, ttl: Duration) -> Vec<String> {
        let mut table = self.inner.write().await;
        let now = Instant::now();
        let stale: Vec<String> = table
            .values()
            .filter(|p| {
                p.source == PeerSource::Discovered
                    && now.duration_since(p.last_seen) > ttl
            })
            .map(|p| p.agent_id.clone())
            .collect();
        for id in &stale {
            table.remove(id);
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
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_static_cfg(id: &str) -> StaticPeerConfig {
        StaticPeerConfig {
            agent_id: id.to_string(),
            addr: "127.0.0.1:7100".parse().expect("addr"),
            pubkey: "Zm9v".to_string(),
        }
    }

    fn make_known_peer(id: &str) -> KnownPeer {
        KnownPeer {
            agent_id: id.to_string(),
            addr: "127.0.0.1:7100".parse().expect("addr"),
            pubkey: "Zm9v".to_string(),
            last_seen_unix_ms: 12345,
        }
    }

    #[tokio::test]
    async fn static_peer_insert_and_lookup() {
        let table = PeerTable::new();
        let cfg = make_static_cfg("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        table.upsert_static(&cfg).await;

        let peer = table.get("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").await.expect("peer exists");
        assert_eq!(peer.source, PeerSource::Static);
        assert_eq!(peer.status, ConnectionStatus::Discovered);
    }

    #[tokio::test]
    async fn discovered_peer_refreshes_last_seen() {
        let table = PeerTable::new();
        let id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        table
            .upsert_discovered(id.to_string(), "127.0.0.1:7101".parse().unwrap(), "YmFy".to_string())
            .await;

        tokio::time::sleep(Duration::from_millis(10)).await;

        table
            .upsert_discovered(id.to_string(), "127.0.0.1:7102".parse().unwrap(), "YmF6".to_string())
            .await;

        let peer = table.get(id).await.expect("peer exists");
        assert_eq!(peer.addr.to_string(), "127.0.0.1:7102");
        assert_eq!(peer.pubkey, "YmF6");
    }

    #[tokio::test]
    async fn discovered_does_not_overwrite_static_addr() {
        let table = PeerTable::new();
        let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

        table.upsert_static(&make_static_cfg(id)).await;

        table
            .upsert_discovered(id.to_string(), "10.0.0.1:9999".parse().unwrap(), "different".to_string())
            .await;

        let peer = table.get(id).await.expect("peer exists");
        assert_eq!(peer.source, PeerSource::Static);
        assert_eq!(peer.addr.to_string(), "127.0.0.1:7100");
    }

    #[tokio::test]
    async fn stale_cleanup_removes_discovered_only() {
        let table = PeerTable::new();

        table.upsert_static(&make_static_cfg("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")).await;

        table
            .upsert_discovered(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
                "127.0.0.1:7101".parse().unwrap(),
                "YmFy".to_string(),
            )
            .await;

        // Manually backdate the discovered peer
        {
            let mut inner = table.inner.write().await;
            if let Some(peer) = inner.get_mut("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb") {
                peer.last_seen = Instant::now() - Duration::from_secs(120);
            }
        }

        let removed = table.remove_stale(Duration::from_secs(60)).await;
        assert_eq!(removed, vec!["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string()]);
        assert!(table.get("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").await.is_some());
        assert!(table.get("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").await.is_none());
    }

    #[tokio::test]
    async fn stale_cleanup_keeps_cached_peers() {
        let table = PeerTable::new();
        let id = "cccccccccccccccccccccccccccccccc";
        table.upsert_cached(&make_known_peer(id)).await;

        {
            let mut inner = table.inner.write().await;
            if let Some(peer) = inner.get_mut(id) {
                peer.last_seen = Instant::now() - Duration::from_secs(120);
            }
        }

        let removed = table.remove_stale(Duration::from_secs(60)).await;
        assert!(removed.is_empty());
        assert!(table.get(id).await.is_some());
    }

    #[tokio::test]
    async fn cached_does_not_overwrite_existing() {
        let table = PeerTable::new();
        let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

        table.upsert_static(&make_static_cfg(id)).await;
        table.upsert_cached(&make_known_peer(id)).await;

        let peer = table.get(id).await.expect("peer exists");
        assert_eq!(peer.source, PeerSource::Static);
    }

    #[tokio::test]
    async fn status_transitions() {
        let table = PeerTable::new();
        let id = "dddddddddddddddddddddddddddddddd";

        table
            .upsert_discovered(id.to_string(), "127.0.0.1:7100".parse().unwrap(), "YmFy".to_string())
            .await;

        table.set_status(id, ConnectionStatus::Connecting).await;
        assert_eq!(table.get(id).await.unwrap().status, ConnectionStatus::Connecting);

        table.set_connected(id, Some(0.7)).await;
        let peer = table.get(id).await.unwrap();
        assert_eq!(peer.status, ConnectionStatus::Connected);
        assert_eq!(peer.rtt_ms, Some(0.7));

        table.set_disconnected(id).await;
        let peer = table.get(id).await.unwrap();
        assert_eq!(peer.status, ConnectionStatus::Disconnected);
        assert_eq!(peer.rtt_ms, None);
    }

    #[tokio::test]
    async fn set_rtt_updates_rtt() {
        let table = PeerTable::new();
        let id = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        table
            .upsert_discovered(id.to_string(), "127.0.0.1:7100".parse().unwrap(), "Zm9v".to_string())
            .await;

        table.set_rtt(id, 0.42).await;
        assert_eq!(table.get(id).await.unwrap().rtt_ms, Some(0.42));
    }

    #[tokio::test]
    async fn touch_refreshes_last_seen() {
        let table = PeerTable::new();
        let id = "ffffffffffffffffffffffffffffffff";
        table
            .upsert_discovered(id.to_string(), "127.0.0.1:7100".parse().unwrap(), "Zm9v".to_string())
            .await;

        {
            let mut inner = table.inner.write().await;
            if let Some(peer) = inner.get_mut(id) {
                peer.last_seen = Instant::now() - Duration::from_secs(120);
            }
        }

        table.touch(id).await;
        let peer = table.get(id).await.unwrap();
        assert!(peer.last_seen.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn list_returns_sorted() {
        let table = PeerTable::new();
        table
            .upsert_discovered("cccc".to_string(), "127.0.0.1:7100".parse().unwrap(), "a".to_string())
            .await;
        table
            .upsert_discovered("aaaa".to_string(), "127.0.0.1:7101".parse().unwrap(), "b".to_string())
            .await;
        table
            .upsert_discovered("bbbb".to_string(), "127.0.0.1:7102".parse().unwrap(), "c".to_string())
            .await;

        let peers = table.list().await;
        assert_eq!(peers.len(), 3);
        assert_eq!(peers[0].agent_id, "aaaa");
        assert_eq!(peers[1].agent_id, "bbbb");
        assert_eq!(peers[2].agent_id, "cccc");
    }

    #[tokio::test]
    async fn remove_returns_removed_peer() {
        let table = PeerTable::new();
        let id = "aaaa";
        table
            .upsert_discovered(id.to_string(), "127.0.0.1:7100".parse().unwrap(), "Zm9v".to_string())
            .await;

        let removed = table.remove(id).await;
        assert!(removed.is_some());
        assert!(table.get(id).await.is_none());
        assert!(table.remove(id).await.is_none());
    }

    #[tokio::test]
    async fn peers_needing_connection() {
        let table = PeerTable::new();
        table
            .upsert_discovered("peer1".to_string(), "127.0.0.1:7100".parse().unwrap(), "a".to_string())
            .await;
        table
            .upsert_discovered("peer2".to_string(), "127.0.0.1:7101".parse().unwrap(), "b".to_string())
            .await;

        table.set_connected("peer2", Some(1.0)).await;

        let needing = table.peers_needing_connection().await;
        assert_eq!(needing.len(), 1);
        assert_eq!(needing[0].agent_id, "peer1");
    }

    #[tokio::test]
    async fn concurrent_access() {
        let table = PeerTable::new();
        let mut handles = Vec::new();

        for i in 0..10 {
            let t = table.clone();
            handles.push(tokio::spawn(async move {
                let id = format!("peer{i:02}");
                t.upsert_discovered(id.clone(), "127.0.0.1:7100".parse().unwrap(), "Zm9v".to_string())
                    .await;
                t.set_connected(&id, Some(i as f64)).await;
                t.touch(&id).await;
            }));
        }

        for i in 0..10 {
            let t = table.clone();
            handles.push(tokio::spawn(async move {
                let _ = t.list().await;
                let _ = t.get(&format!("peer{i:02}")).await;
                let _ = t.peers_needing_connection().await;
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(table.list().await.len(), 10);
    }
}
