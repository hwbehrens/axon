use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

struct ReplayCacheInner {
    seen: HashMap<uuid::Uuid, Instant>,
    order: VecDeque<(uuid::Uuid, Instant)>,
}

pub(crate) struct ReplayCache {
    ttl: Duration,
    max_entries: usize,
    inner: tokio::sync::Mutex<ReplayCacheInner>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReplayCacheEntry {
    id: uuid::Uuid,
    seen_at_ms: u64,
}

impl ReplayCache {
    #[cfg(test)]
    pub(crate) fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            ttl,
            max_entries,
            inner: tokio::sync::Mutex::new(ReplayCacheInner {
                seen: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    pub(crate) fn load(path: &std::path::Path, ttl: Duration, max_entries: usize) -> Self {
        let mut seen = HashMap::new();
        let mut order = VecDeque::new();
        if let Ok(data) = std::fs::read_to_string(path)
            && let Ok(entries) = serde_json::from_str::<Vec<ReplayCacheEntry>>(&data)
        {
            let now_ms = crate::message::now_millis();
            let ttl_ms = ttl.as_millis() as u64;
            let now_instant = Instant::now();
            for entry in entries {
                if now_ms.saturating_sub(entry.seen_at_ms) <= ttl_ms {
                    let age = Duration::from_millis(now_ms.saturating_sub(entry.seen_at_ms));
                    let ts = now_instant - age;
                    seen.insert(entry.id, ts);
                    order.push_back((entry.id, ts));
                }
            }
        }
        Self {
            ttl,
            max_entries,
            inner: tokio::sync::Mutex::new(ReplayCacheInner { seen, order }),
        }
    }

    pub(crate) async fn save(&self, path: &std::path::Path) -> Result<()> {
        let inner = self.inner.lock().await;
        let now = Instant::now();
        let now_ms = crate::message::now_millis();
        let entries: Vec<ReplayCacheEntry> = inner
            .seen
            .iter()
            .filter(|(_, ts)| now.saturating_duration_since(**ts) <= self.ttl)
            .map(|(id, ts)| {
                let age_ms = now.saturating_duration_since(*ts).as_millis() as u64;
                ReplayCacheEntry {
                    id: *id,
                    seen_at_ms: now_ms.saturating_sub(age_ms),
                }
            })
            .collect();
        drop(inner);
        let data = serde_json::to_vec_pretty(&entries).context("failed to encode replay cache")?;
        tokio::fs::write(path, data)
            .await
            .with_context(|| format!("failed to write replay cache: {}", path.display()))?;
        Ok(())
    }

    pub(crate) async fn is_replay(&self, id: uuid::Uuid, now: Instant) -> bool {
        let mut inner = self.inner.lock().await;
        // Drain only expired entries from the front
        while let Some(&(front_id, front_ts)) = inner.order.front() {
            if now.saturating_duration_since(front_ts) > self.ttl {
                inner.order.pop_front();
                // Only remove from map if timestamp matches (handles re-insertion)
                if let Some(&ts) = inner.seen.get(&front_id)
                    && ts == front_ts
                {
                    inner.seen.remove(&front_id);
                }
            } else {
                break;
            }
        }
        if inner.seen.contains_key(&id) {
            return true;
        }
        inner.seen.insert(id, now);
        inner.order.push_back((id, now));
        // Evict oldest entries if over capacity
        while inner.seen.len() > self.max_entries {
            if let Some((old_id, old_ts)) = inner.order.pop_front() {
                if let Some(&ts) = inner.seen.get(&old_id)
                    && ts == old_ts
                {
                    inner.seen.remove(&old_id);
                }
            } else {
                break;
            }
        }
        false
    }
}

#[cfg(test)]
#[path = "replay_cache_tests.rs"]
mod tests;
