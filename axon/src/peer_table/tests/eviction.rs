use super::super::*;
use std::time::Duration;

#[tokio::test]
async fn upsert_discovered_evicts_stale_cached_at_same_addr() {
    let table = PeerTable::new();
    let addr: std::net::SocketAddr = "10.0.0.1:7100".parse().unwrap();

    // Insert a cached peer at addr
    table
        .upsert_cached(&KnownPeer {
            agent_id: "old_peer".into(),
            addr,
            pubkey: "b2xk".to_string(),
            last_seen_unix_ms: 1000,
        })
        .await;
    assert!(table.get("old_peer").await.is_some());

    // Discover a new peer at the same addr
    table
        .upsert_discovered("new_peer".into(), addr, "bmV3".to_string())
        .await;

    assert!(
        table.get("old_peer").await.is_none(),
        "stale peer should be evicted"
    );
    assert!(
        table.get("new_peer").await.is_some(),
        "new peer should exist"
    );

    // Pubkey map should only have new peer
    let map = table.pubkey_map();
    let map = map.read().unwrap();
    assert!(!map.contains_key("old_peer"));
    assert!(map.contains_key("new_peer"));
}

#[tokio::test]
async fn upsert_discovered_does_not_evict_static_at_same_addr() {
    let table = PeerTable::new();
    let addr: std::net::SocketAddr = "10.0.0.1:7100".parse().unwrap();

    table
        .upsert_static(&StaticPeerConfig {
            agent_id: "static_peer".into(),
            addr,
            pubkey: "c3RhdGlj".to_string(),
        })
        .await;

    table
        .upsert_discovered("new_peer".into(), addr, "bmV3".to_string())
        .await;

    assert!(
        table.get("static_peer").await.is_some(),
        "static peer must not be evicted"
    );
    assert!(
        table.get("new_peer").await.is_none(),
        "discovered peer should be blocked by static at same addr"
    );
}

#[tokio::test]
async fn upsert_cached_evicts_stale_discovered_at_same_addr() {
    let table = PeerTable::new();
    let addr: std::net::SocketAddr = "10.0.0.1:7100".parse().unwrap();

    table
        .upsert_discovered("old_peer".into(), addr, "b2xk".to_string())
        .await;

    table
        .upsert_cached(&KnownPeer {
            agent_id: "new_peer".into(),
            addr,
            pubkey: "bmV3".to_string(),
            last_seen_unix_ms: 2000,
        })
        .await;

    assert!(
        table.get("old_peer").await.is_none(),
        "stale peer should be evicted"
    );
    assert!(
        table.get("new_peer").await.is_some(),
        "new peer should exist"
    );
}

#[tokio::test]
async fn stale_cleanup_boundary_exactly_at_ttl() {
    let table = PeerTable::new();
    let id = "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let ttl = Duration::from_secs(60);

    table
        .upsert_discovered(
            id.into(),
            "127.0.0.1:7101".parse().unwrap(),
            "YmFy".to_string(),
        )
        .await;

    let beyond_id = "ed25519.cccccccccccccccccccccccccccccccc";
    table
        .upsert_discovered(
            beyond_id.into(),
            "127.0.0.1:7102".parse().unwrap(),
            "YmF6".to_string(),
        )
        .await;

    // Set one peer to just within TTL (should survive) and one well beyond (should be removed).
    // Use a 1-second margin to avoid flakiness from clock advancement between set and check.
    {
        let mut inner = table.inner.write().await;
        if let Some(peer) = inner.get_mut(id) {
            peer.last_seen = Instant::now() - ttl + Duration::from_secs(1);
        }
        if let Some(peer) = inner.get_mut(beyond_id) {
            peer.last_seen = Instant::now() - ttl - Duration::from_secs(10);
        }
    }

    let removed = table.remove_stale(ttl).await;
    assert!(
        !removed.contains(&AgentId::from(id)),
        "peer within TTL should NOT be removed"
    );
    assert!(
        removed.contains(&AgentId::from(beyond_id)),
        "peer beyond TTL should be removed"
    );
    assert!(table.get(id).await.is_some());
}

#[tokio::test]
async fn evict_stale_at_addr_evicts_all_matching_peers() {
    let table = PeerTable::new();
    let addr: std::net::SocketAddr = "10.0.0.1:7100".parse().unwrap();

    // Insert 3 discovered peers at the same address by writing directly
    // (upsert_discovered would evict prior peers at the same addr)
    {
        let mut inner = table.inner.write().await;
        for id in &["peer_a", "peer_b", "peer_c"] {
            inner.insert(
                AgentId::from(*id),
                PeerRecord {
                    agent_id: AgentId::from(*id),
                    addr,
                    pubkey: format!("{id}_key"),
                    source: PeerSource::Discovered,
                    status: ConnectionStatus::Discovered,
                    rtt_ms: None,
                    last_seen: Instant::now(),
                },
            );
        }
    }
    {
        let pmap = table.pubkey_map();
        let mut map = pmap.write().unwrap();
        for id in &["peer_a", "peer_b", "peer_c"] {
            map.insert(id.to_string(), format!("{id}_key"));
        }
    }
    assert_eq!(table.list().await.len(), 3);

    // Discover a 4th peer at the same address — should evict all 3
    table
        .upsert_discovered("peer_new".into(), addr, "new_key".to_string())
        .await;

    assert_eq!(table.list().await.len(), 1);
    assert!(table.get("peer_new").await.is_some());
    for id in &["peer_a", "peer_b", "peer_c"] {
        assert!(table.get(id).await.is_none(), "{id} should be evicted");
    }

    let map = table.pubkey_map();
    let map = map.read().unwrap();
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("peer_new"));
}

#[tokio::test]
async fn upsert_discovered_blocked_by_static_at_same_addr() {
    let table = PeerTable::new();
    let addr: std::net::SocketAddr = "10.0.0.1:7100".parse().unwrap();

    table
        .upsert_static(&StaticPeerConfig {
            agent_id: "static_peer".into(),
            addr,
            pubkey: "c3RhdGlj".to_string(),
        })
        .await;

    // Try to discover a different peer at the same address — should be blocked
    table
        .upsert_discovered("discovered_peer".into(), addr, "bmV3".to_string())
        .await;

    assert!(
        table.get("static_peer").await.is_some(),
        "static peer must remain"
    );
    assert!(
        table.get("discovered_peer").await.is_none(),
        "discovered peer should be blocked by static at same addr"
    );

    let map = table.pubkey_map();
    let map = map.read().unwrap();
    assert!(!map.contains_key("discovered_peer"));
}

#[tokio::test]
async fn upsert_cached_blocked_by_static_at_same_addr() {
    let table = PeerTable::new();
    let addr: std::net::SocketAddr = "10.0.0.1:7100".parse().unwrap();

    table
        .upsert_static(&StaticPeerConfig {
            agent_id: "static_peer".into(),
            addr,
            pubkey: "c3RhdGlj".to_string(),
        })
        .await;

    table
        .upsert_cached(&KnownPeer {
            agent_id: "cached_peer".into(),
            addr,
            pubkey: "bmV3".to_string(),
            last_seen_unix_ms: 5000,
        })
        .await;

    assert!(
        table.get("static_peer").await.is_some(),
        "static peer must remain"
    );
    assert!(
        table.get("cached_peer").await.is_none(),
        "cached peer should be blocked by static at same addr"
    );
}

#[tokio::test]
async fn upsert_static_evicts_all_at_same_addr() {
    let table = PeerTable::new();
    let addr: std::net::SocketAddr = "10.0.0.1:7100".parse().unwrap();

    table
        .upsert_discovered("disc_a".into(), addr, "key_a".to_string())
        .await;
    table
        .upsert_cached(&KnownPeer {
            agent_id: "cached_b".into(),
            addr,
            pubkey: "key_b".to_string(),
            last_seen_unix_ms: 1000,
        })
        .await;

    // Static peer at same addr — should evict both
    table
        .upsert_static(&StaticPeerConfig {
            agent_id: "static_new".into(),
            addr,
            pubkey: "static_key".to_string(),
        })
        .await;

    assert_eq!(table.list().await.len(), 1);
    assert!(table.get("static_new").await.is_some());
    assert!(table.get("disc_a").await.is_none());
    assert!(table.get("cached_b").await.is_none());

    let map = table.pubkey_map();
    let map = map.read().unwrap();
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("static_new"));
}
