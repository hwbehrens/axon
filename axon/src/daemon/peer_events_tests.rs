use super::*;
use crate::config::StaticPeerConfig;
use crate::peer_table::{PeerSource, PeerTable};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Instant;

fn addr(value: &str) -> SocketAddr {
    value.parse().expect("socket addr")
}

#[tokio::test]
async fn lost_discovered_peer_is_removed_from_table() {
    let table = PeerTable::new();
    let agent_id: AgentId = "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into();
    table
        .upsert_discovered(agent_id.clone(), addr("127.0.0.1:7100"), "Zm9v".to_string())
        .await;

    let mut reconnect_state = HashMap::new();
    reconnect_state.insert(agent_id.clone(), ReconnectState::immediate(Instant::now()));

    handle_peer_event(
        PeerEvent::Lost {
            agent_id: agent_id.clone(),
        },
        &table,
        &mut reconnect_state,
    )
    .await;

    assert!(table.get(agent_id.as_str()).await.is_none());
    assert!(!reconnect_state.contains_key(&agent_id));
}

#[tokio::test]
async fn lost_static_peer_is_ignored() {
    let table = PeerTable::new();
    let cfg = StaticPeerConfig {
        agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        addr: addr("127.0.0.1:7100"),
        pubkey: "Zm9v".to_string(),
    };
    table.upsert_static(&cfg).await;

    let mut reconnect_state = HashMap::new();
    handle_peer_event(
        PeerEvent::Lost {
            agent_id: cfg.agent_id.clone(),
        },
        &table,
        &mut reconnect_state,
    )
    .await;

    assert!(table.get(cfg.agent_id.as_str()).await.is_some());
}

#[tokio::test]
async fn discovered_event_refreshes_static_addr_when_pubkey_matches() {
    let table = PeerTable::new();
    let cfg = StaticPeerConfig {
        agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        addr: addr("127.0.0.1:7100"),
        pubkey: "Zm9v".to_string(),
    };
    table.upsert_static(&cfg).await;

    let mut reconnect_state = HashMap::new();
    handle_peer_event(
        PeerEvent::Discovered {
            agent_id: cfg.agent_id.clone(),
            addr: addr("127.0.0.1:7200"),
            pubkey: cfg.pubkey.clone(),
        },
        &table,
        &mut reconnect_state,
    )
    .await;

    let peer = table.get(cfg.agent_id.as_str()).await.expect("peer exists");
    assert_eq!(peer.addr, addr("127.0.0.1:7200"));
    assert_eq!(peer.source, PeerSource::Static);
    assert!(reconnect_state.contains_key(&cfg.agent_id));
}
