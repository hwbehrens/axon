use super::*;
use crate::config::AxonPaths;
use crate::identity::Identity;
use crate::message::MessageKind;
use crate::peer_table::PeerTable;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::tempdir;

async fn make_transport_pair() -> (
    Identity,
    Identity,
    QuicTransport,
    QuicTransport,
    PeerTable,
    PeerTable,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let dir_a = tempdir().expect("tempdir a");
    let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
    let id_a = Identity::load_or_generate(&paths_a).expect("identity a");

    let dir_b = tempdir().expect("tempdir b");
    let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
    let id_b = Identity::load_or_generate(&paths_b).expect("identity b");

    let table_a = PeerTable::new();
    let table_b = PeerTable::new();

    // Register each peer's pubkey in the other's table
    table_a
        .upsert_discovered(
            id_b.agent_id().into(),
            "127.0.0.1:1".parse().unwrap(), // placeholder, overwritten by ensure_connection
            id_b.public_key_base64().to_string(),
        )
        .await;
    table_b
        .upsert_discovered(
            id_a.agent_id().into(),
            "127.0.0.1:1".parse().unwrap(),
            id_a.public_key_base64().to_string(),
        )
        .await;

    let transport_b = QuicTransport::bind(
        "127.0.0.1:0".parse().unwrap(),
        &id_b,
        128,
        table_b.pubkey_map(),
    )
    .await
    .expect("bind b");

    let transport_a = QuicTransport::bind(
        "127.0.0.1:0".parse().unwrap(),
        &id_a,
        128,
        table_a.pubkey_map(),
    )
    .await
    .expect("bind a");

    (
        id_a,
        id_b,
        transport_a,
        transport_b,
        table_a,
        table_b,
        dir_a,
        dir_b,
    )
}

#[tokio::test]
async fn endpoint_binds_and_reports_addr() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let table = PeerTable::new();

    let transport = QuicTransport::bind(
        "127.0.0.1:0".parse().unwrap(),
        &identity,
        128,
        table.pubkey_map(),
    )
    .await
    .expect("bind");

    let addr = transport.local_addr().expect("local_addr");
    assert_eq!(addr.ip(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
    assert_ne!(addr.port(), 0);
}

#[tokio::test]
async fn two_peers_connect() {
    let (_id_a, id_b, transport_a, transport_b, _, _, _dir_a, _dir_b) = make_transport_pair().await;
    let addr_b = transport_b.local_addr().expect("local_addr b");

    let peer_b = PeerRecord {
        agent_id: id_b.agent_id().into(),
        addr: addr_b,
        pubkey: id_b.public_key_base64().to_string(),
        source: crate::peer_table::PeerSource::Static,
        status: crate::peer_table::ConnectionStatus::Discovered,
        rtt_ms: None,
        last_seen: std::time::Instant::now(),
    };

    let conn = transport_a
        .ensure_connection(&peer_b)
        .await
        .expect("connect");
    assert!(conn.close_reason().is_none());
    assert!(transport_a.has_connection(id_b.agent_id()).await);
}

#[tokio::test]
async fn send_notify_unidirectional() {
    let (id_a, id_b, transport_a, transport_b, _, _, _dir_a, _dir_b) = make_transport_pair().await;
    let addr_b = transport_b.local_addr().expect("local_addr b");
    let mut rx_b = transport_b.subscribe_inbound();

    let peer_b = PeerRecord {
        agent_id: id_b.agent_id().into(),
        addr: addr_b,
        pubkey: id_b.public_key_base64().to_string(),
        source: crate::peer_table::PeerSource::Static,
        status: crate::peer_table::ConnectionStatus::Discovered,
        rtt_ms: None,
        last_seen: std::time::Instant::now(),
    };

    let notify = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Message,
        json!({"topic": "test", "data": {"msg": "hello"}}),
    );

    let result = transport_a
        .send(&peer_b, notify.clone())
        .await
        .expect("send");
    assert!(result.is_none());

    let received = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
        .await
        .expect("timeout waiting for inbound")
        .expect("recv");
    assert_eq!(received.kind, MessageKind::Message);
    assert_eq!(received.from.as_deref(), Some(id_a.agent_id()));
}

#[tokio::test]
async fn send_request_bidirectional_default_error() {
    let (id_a, id_b, transport_a, transport_b, _, _, _dir_a, _dir_b) = make_transport_pair().await;
    let addr_b = transport_b.local_addr().expect("local_addr b");

    let peer_b = PeerRecord {
        agent_id: id_b.agent_id().into(),
        addr: addr_b,
        pubkey: id_b.public_key_base64().to_string(),
        source: crate::peer_table::PeerSource::Static,
        status: crate::peer_table::ConnectionStatus::Discovered,
        rtt_ms: None,
        last_seen: std::time::Instant::now(),
    };

    let request = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Request,
        json!({"question": "test?"}),
    );

    let result = transport_a
        .send(&peer_b, request.clone())
        .await
        .expect("send");
    let response = result.expect("expected response");
    assert_eq!(response.kind, MessageKind::Error);
    assert_eq!(response.ref_id, Some(request.id));
}
