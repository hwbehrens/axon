//! Runtime cache of capability manifests observed from connected peers.
//!
//! Entries are *claims* pulled via `describe` exchanges; they are advisory
//! views that age out and never grant authority. The cache is bounded and
//! evicts oldest-inserted entries; re-insertion refreshes recency.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::message::AgentId;

use super::types::Manifest;

/// Upper bound on cached peer manifests. Generous for any realistic LAN;
/// prevents unbounded growth if enrollment churns.
pub const MAX_MANIFEST_CACHE_ENTRIES: usize = 256;

#[derive(Debug)]
struct CachedManifest {
    manifest: Arc<Manifest>,
    fetched_at: Instant,
}

#[derive(Debug, Default)]
struct CacheState {
    map: HashMap<AgentId, CachedManifest>,
    order: VecDeque<AgentId>,
}

/// Bounded, TTL-aware cache of remote peer manifests.
#[derive(Debug, Clone, Default)]
pub struct ManifestCache {
    state: Arc<Mutex<CacheState>>,
}

impl ManifestCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store (or refresh) the manifest for a peer.
    pub async fn insert(&self, agent_id: AgentId, manifest: Manifest) {
        let mut state = self.state.lock().await;
        state.map.insert(
            agent_id.clone(),
            CachedManifest {
                manifest: Arc::new(manifest),
                fetched_at: Instant::now(),
            },
        );
        // Refresh recency ordering for existing keys.
        state.order.retain(|id| id != &agent_id);
        state.order.push_back(agent_id);
        while state.map.len() > MAX_MANIFEST_CACHE_ENTRIES {
            let Some(oldest) = state.order.pop_front() else {
                break;
            };
            state.map.remove(&oldest);
        }
    }

    /// Return the manifest when a fresh entry exists (no re-pull needed).
    pub async fn fresh(&self, agent_id: &AgentId, ttl: Duration) -> Option<Arc<Manifest>> {
        let state = self.state.lock().await;
        state
            .map
            .get(agent_id)
            .filter(|cached| cached.fetched_at.elapsed() <= ttl)
            .map(|cached| Arc::clone(&cached.manifest))
    }

    /// Return the cached manifest regardless of age (advisory display only).
    pub async fn get(&self, agent_id: &AgentId) -> Option<Arc<Manifest>> {
        let state = self.state.lock().await;
        state.map.get(agent_id).map(|c| Arc::clone(&c.manifest))
    }

    /// Drop one peer's entry (e.g. on revocation or disconnect).
    pub async fn remove(&self, agent_id: &AgentId) {
        let mut state = self.state.lock().await;
        state.map.remove(agent_id);
        state.order.retain(|id| id != agent_id);
    }

    pub async fn len(&self) -> usize {
        self.state.lock().await.map.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}
