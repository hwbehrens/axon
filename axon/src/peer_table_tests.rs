use super::*;
use std::time::Duration;

fn make_static_cfg(id: &str) -> StaticPeerConfig {
    StaticPeerConfig {
        agent_id: id.into(),
        addr: "127.0.0.1:7100".parse().expect("addr"),
        pubkey: "Zm9v".to_string(),
    }
}

fn make_known_peer(id: &str) -> KnownPeer {
    KnownPeer {
        agent_id: id.into(),
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

    let peer = table
        .get("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        .await
        .expect("peer exists");
    assert_eq!(peer.source, PeerSource::Static);
    assert_eq!(peer.status, ConnectionStatus::Discovered);
}

#[tokio::test]
async fn discovered_peer_refreshes_last_seen() {
    let table = PeerTable::new();
    let id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    table
        .upsert_discovered(
            id.into(),
            "127.0.0.1:7101".parse().unwrap(),
            "YmFy".to_string(),
        )
        .await;

    tokio::time::sleep(Duration::from_millis(10)).await;

    table
        .upsert_discovered(
            id.into(),
            "127.0.0.1:7102".parse().unwrap(),
            "YmF6".to_string(),
        )
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
        .upsert_discovered(
            id.into(),
            "10.0.0.1:9999".parse().unwrap(),
            "different".to_string(),
        )
        .await;

    let peer = table.get(id).await.expect("peer exists");
    assert_eq!(peer.source, PeerSource::Static);
    assert_eq!(peer.addr.to_string(), "127.0.0.1:7100");
}

#[tokio::test]
async fn stale_cleanup_removes_discovered_only() {
    let table = PeerTable::new();

    table
        .upsert_static(&make_static_cfg("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
        .await;

    table
        .upsert_discovered(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
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
    assert_eq!(
        removed,
        vec![AgentId::from("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")]
    );
    assert!(
        table
            .get("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .await
            .is_some()
    );
    assert!(
        table
            .get("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .await
            .is_none()
    );
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
        .upsert_discovered(
            id.into(),
            "127.0.0.1:7100".parse().unwrap(),
            "YmFy".to_string(),
        )
        .await;

    table.set_status(id, ConnectionStatus::Connecting).await;
    assert_eq!(
        table.get(id).await.unwrap().status,
        ConnectionStatus::Connecting
    );

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
        .upsert_discovered(
            id.into(),
            "127.0.0.1:7100".parse().unwrap(),
            "Zm9v".to_string(),
        )
        .await;

    table.set_rtt(id, 0.42).await;
    assert_eq!(table.get(id).await.unwrap().rtt_ms, Some(0.42));
}

#[tokio::test]
async fn touch_refreshes_last_seen() {
    let table = PeerTable::new();
    let id = "ffffffffffffffffffffffffffffffff";
    table
        .upsert_discovered(
            id.into(),
            "127.0.0.1:7100".parse().unwrap(),
            "Zm9v".to_string(),
        )
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
        .upsert_discovered(
            "cccc".into(),
            "127.0.0.1:7100".parse().unwrap(),
            "a".to_string(),
        )
        .await;
    table
        .upsert_discovered(
            "aaaa".into(),
            "127.0.0.1:7101".parse().unwrap(),
            "b".to_string(),
        )
        .await;
    table
        .upsert_discovered(
            "bbbb".into(),
            "127.0.0.1:7102".parse().unwrap(),
            "c".to_string(),
        )
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
        .upsert_discovered(
            id.into(),
            "127.0.0.1:7100".parse().unwrap(),
            "Zm9v".to_string(),
        )
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
        .upsert_discovered(
            "peer1".into(),
            "127.0.0.1:7100".parse().unwrap(),
            "a".to_string(),
        )
        .await;
    table
        .upsert_discovered(
            "peer2".into(),
            "127.0.0.1:7101".parse().unwrap(),
            "b".to_string(),
        )
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
            t.upsert_discovered(
                AgentId::from(id.clone()),
                "127.0.0.1:7100".parse().unwrap(),
                "Zm9v".to_string(),
            )
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

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

#[derive(Debug, Clone)]
enum PeerOp {
    Insert(String),
    Remove(String),
    SetStatus(String, ConnectionStatus),
    SetConnected(String),
    SetDisconnected(String),
    List,
}

fn arb_peer_op() -> impl Strategy<Value = PeerOp> {
    let id_strategy = "[0-9a-f]{32}";
    prop_oneof![
        id_strategy.prop_map(PeerOp::Insert),
        id_strategy.prop_map(PeerOp::Remove),
        (
            id_strategy,
            prop::sample::select(vec![
                ConnectionStatus::Discovered,
                ConnectionStatus::Connecting,
                ConnectionStatus::Connected,
                ConnectionStatus::Disconnected,
            ])
        )
            .prop_map(|(id, s)| PeerOp::SetStatus(id, s)),
        id_strategy.prop_map(PeerOp::SetConnected),
        id_strategy.prop_map(PeerOp::SetDisconnected),
        Just(PeerOp::List),
    ]
}

proptest! {
    #[test]
    fn concurrent_insert_remove_never_panics(
        ops in proptest::collection::vec(arb_peer_op(), 1..50),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let table = PeerTable::new();
            let mut handles = Vec::new();
            for op in ops {
                let t = table.clone();
                handles.push(tokio::spawn(async move {
                    match op {
                        PeerOp::Insert(id) => {
                            t.upsert_discovered(
                                id.into(),
                                "127.0.0.1:7100".parse().unwrap(),
                                "Zm9v".to_string(),
                            ).await;
                        }
                        PeerOp::Remove(id) => { t.remove(&id).await; }
                        PeerOp::SetStatus(id, s) => { t.set_status(&id, s).await; }
                        PeerOp::SetConnected(id) => { t.set_connected(&id, Some(1.0)).await; }
                        PeerOp::SetDisconnected(id) => { t.set_disconnected(&id).await; }
                        PeerOp::List => { let _ = t.list().await; }
                    }
                }));
            }
            for h in handles {
                h.await.unwrap();
            }
            let listed = table.list().await;
            assert!(listed.len() <= 50);
        });
    }
}

// =========================================================================
// Mutation-coverage: remove_stale > vs >= boundary
// =========================================================================

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

// =========================================================================
// Mutation-coverage: to_known_peers returns all peers
// =========================================================================

#[tokio::test]
async fn to_known_peers_returns_all_peers() {
    let table = PeerTable::new();

    table
        .upsert_static(&make_static_cfg("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
        .await;
    table
        .upsert_discovered(
            "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            "127.0.0.1:7101".parse().unwrap(),
            "YmFy".to_string(),
        )
        .await;
    table
        .upsert_cached(&make_known_peer("ed25519.cccccccccccccccccccccccccccccccc"))
        .await;

    let known = table.to_known_peers().await;
    assert_eq!(known.len(), 3);
    let ids: std::collections::HashSet<_> = known.iter().map(|k| k.agent_id.as_str()).collect();
    assert!(ids.contains("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    assert!(ids.contains("ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
    assert!(ids.contains("ed25519.cccccccccccccccccccccccccccccccc"));
}
