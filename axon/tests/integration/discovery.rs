use crate::*;

// =========================================================================
// Discovery → PeerTable integration
// =========================================================================

/// run_static_discovery emits PeerEvents that feed PeerTable correctly.
#[tokio::test]
async fn static_discovery_feeds_peer_table() {
    let peers = vec![
        StaticPeerConfig {
            agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            addr: "10.0.0.1:7100".parse().unwrap(),
            pubkey: "Zm9v".to_string(),
        },
        StaticPeerConfig {
            agent_id: "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            addr: "10.0.0.2:7100".parse().unwrap(),
            pubkey: "YmFy".to_string(),
        },
    ];

    let (tx, mut rx) = mpsc::channel(16);

    tokio::spawn(async move {
        let _ = run_static_discovery(peers, tx, CancellationToken::new()).await;
    });

    let table = PeerTable::new();

    // Drain discovery events into the peer table.
    for _ in 0..2 {
        match rx.recv().await.unwrap() {
            PeerEvent::Discovered {
                agent_id,
                addr,
                pubkey,
            } => {
                table.upsert_discovered(agent_id, addr, pubkey).await;
            }
            _ => panic!("expected Discovered"),
        }
    }

    let all = table.list().await;
    assert_eq!(all.len(), 2);
    assert!(
        table
            .get("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .await
            .is_some()
    );
    assert!(
        table
            .get("ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .await
            .is_some()
    );
}

// =========================================================================
// PeerTable lifecycle integration
// =========================================================================

/// Full lifecycle: insert static + discovered, connection state transitions,
/// stale cleanup, known_peers export.
#[tokio::test]
async fn peer_table_full_lifecycle() {
    let table = PeerTable::new();

    // 1. Insert static peer.
    table
        .upsert_static(&StaticPeerConfig {
            agent_id: "static_peer".into(),
            addr: "10.0.0.1:7100".parse().unwrap(),
            pubkey: "Zm9v".to_string(),
        })
        .await;

    // 2. Insert discovered peer.
    table
        .upsert_discovered(
            "discovered_peer".into(),
            "10.0.0.2:7100".parse().unwrap(),
            "YmFy".to_string(),
        )
        .await;

    assert_eq!(table.list().await.len(), 2);

    // 3. Connection transitions.
    table
        .set_status("discovered_peer", ConnectionStatus::Connecting)
        .await;
    assert_eq!(
        table.get("discovered_peer").await.unwrap().status,
        ConnectionStatus::Connecting
    );

    table.set_connected("discovered_peer", Some(1.5)).await;
    let p = table.get("discovered_peer").await.unwrap();
    assert_eq!(p.status, ConnectionStatus::Connected);
    assert_eq!(p.rtt_ms, Some(1.5));

    // 4. Export as known peers.
    let known = table.to_known_peers().await;
    assert_eq!(known.len(), 2);

    // 5. Disconnect and verify status.
    table.set_disconnected("discovered_peer").await;
    let p = table.get("discovered_peer").await.unwrap();
    assert_eq!(p.status, ConnectionStatus::Disconnected);
    assert!(p.rtt_ms.is_none());

    // 6. Remove peer and verify.
    let removed = table.remove("discovered_peer").await;
    assert!(removed.is_some());
    assert!(table.get("discovered_peer").await.is_none());
    assert!(table.get("static_peer").await.is_some());
}
