use std::time::Duration;

use super::{MAX_MANIFEST_CACHE_ENTRIES, Manifest, ManifestCache};
use crate::message::AgentId;

fn agent(value: char) -> AgentId {
    AgentId::parse(&format!("ed25519.{}", value.to_string().repeat(32))).expect("valid Agent ID")
}

fn manifest_named(name: &str) -> Manifest {
    Manifest::from_parts(
        Some(name.to_string()),
        None,
        vec![super::super::ServiceEntry {
            id: "svc".to_string(),
            description: "does things".to_string(),
            example_request: None,
            example_response: None,
            timeout_hint_secs: None,
            concurrency: None,
            errors: None,
        }],
    )
    .expect("valid manifest")
}

#[tokio::test]
async fn fresh_entries_skip_repull_but_expire() {
    let cache = ManifestCache::new();
    let peer = agent('a');
    cache.insert(peer.clone(), manifest_named("forge")).await;

    assert!(
        cache.fresh(&peer, Duration::from_secs(60)).await.is_some(),
        "recently inserted entry is fresh"
    );
    assert!(
        cache.fresh(&peer, Duration::ZERO).await.is_none(),
        "zero TTL forces a repull"
    );
    // Advisory reads ignore age.
    assert!(cache.get(&peer).await.is_some());
}

#[tokio::test]
async fn remove_drops_the_entry() {
    let cache = ManifestCache::new();
    let peer = agent('b');
    cache.insert(peer.clone(), manifest_named("x")).await;
    cache.remove(&peer).await;
    assert!(cache.get(&peer).await.is_none());
    assert!(cache.is_empty().await);
}

#[tokio::test]
async fn insert_evicts_oldest_at_capacity() {
    let cache = ManifestCache::new();
    let first = agent('c');
    cache.insert(first.clone(), manifest_named("first")).await;
    // MAX unique filler peers (hex-encoded indices): at capacity the
    // oldest-inserted entry — `first` — must be the one evicted.
    for i in 0..MAX_MANIFEST_CACHE_ENTRIES {
        let id = AgentId::parse(&format!("ed25519.{:032x}", i)).expect("valid Agent ID");
        cache.insert(id, manifest_named("filler")).await;
    }
    assert_eq!(cache.len().await, MAX_MANIFEST_CACHE_ENTRIES);
    assert!(
        cache.get(&first).await.is_none(),
        "oldest entry must be evicted at capacity"
    );
}

#[tokio::test]
async fn retain_connected_evicts_entries_for_peers_no_longer_connected() {
    let cache = ManifestCache::new();
    let connected = agent('a');
    let gone = agent('b');
    cache.insert(connected.clone(), manifest_named("a")).await;
    cache.insert(gone.clone(), manifest_named("b")).await;

    cache
        .retain_connected(std::slice::from_ref(&connected))
        .await;
    assert!(cache.get(&connected).await.is_some());
    assert!(
        cache.get(&gone).await.is_none(),
        "disconnected peers must not retain advisory service summaries"
    );
    assert_eq!(cache.len().await, 1);
}
