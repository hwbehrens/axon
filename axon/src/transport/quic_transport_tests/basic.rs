use super::super::QuicTransport;
use super::fixtures::{make_transport_pair, peer_record};
use crate::config::AxonPaths;
use crate::identity::Identity;
use crate::message::{Envelope, MessageKind};
use crate::peer_table::PeerTable;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::tempdir;

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
    let pair = make_transport_pair().await;
    let addr_b = pair.transport_b.local_addr().expect("local_addr b");
    let peer_b = peer_record(&pair.id_b, addr_b);

    let conn = pair
        .transport_a
        .ensure_connection(&peer_b)
        .await
        .expect("connect");
    assert!(conn.close_reason().is_none());
    assert!(pair.transport_a.has_connection(pair.id_b.agent_id()).await);
}

#[tokio::test]
async fn send_notify_unidirectional() {
    let pair = make_transport_pair().await;
    let addr_b = pair.transport_b.local_addr().expect("local_addr b");
    let mut rx_b = pair.transport_b.subscribe_inbound();
    let peer_b = peer_record(&pair.id_b, addr_b);

    let notify = Envelope::new(
        pair.id_a.agent_id().to_string(),
        pair.id_b.agent_id().to_string(),
        MessageKind::Message,
        json!({"topic": "test", "data": {"msg": "hello"}}),
    );

    let result = pair
        .transport_a
        .send(&peer_b, notify.clone())
        .await
        .expect("send");
    assert!(result.is_none());

    let received = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
        .await
        .expect("timeout waiting for inbound")
        .expect("recv");
    assert_eq!(received.kind, MessageKind::Message);
    assert_eq!(received.from.as_deref(), Some(pair.id_a.agent_id()));
}
