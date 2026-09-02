use std::time::Duration;

use super::{Manifest, ManifestCache, MAX_MANIFEST_CACHE_ENTRIES};
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
    for i in 0..MAX_MANIFEST_CACHE_ENTRIES {
        let id = agent(char::from(b'd' + (i % 20) as u8));
        cache.insert(id, manifest_named("filler")).await;
    }
    assert_eq!(cache.len().await, MAX_MANIFEST_CACHE_ENTRIES);
    assert!(
        cache.get(&first).await.is_none(),
        "oldest entry must be evicted at capacity"
    );
}
