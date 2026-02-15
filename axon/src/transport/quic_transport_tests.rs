use super::*;
use crate::config::AxonPaths;
use crate::identity::Identity;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn endpoint_binds_and_reports_addr() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");

    let transport = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &identity, 128)
        .await
        .expect("bind");

    let addr = transport.local_addr().expect("local_addr");
    assert_eq!(addr.ip(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
    assert_ne!(addr.port(), 0);
}

#[tokio::test]
async fn two_peers_hello_exchange() {
    let dir_a = tempdir().expect("tempdir a");
    let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
    let id_a = Identity::load_or_generate(&paths_a).expect("identity a");

    let dir_b = tempdir().expect("tempdir b");
    let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
    let id_b = Identity::load_or_generate(&paths_b).expect("identity b");

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .expect("bind b");
    let addr_b = transport_b.local_addr().expect("local_addr b");

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .expect("bind a");

    let peer_b = PeerRecord {
        agent_id: id_b.agent_id().to_string(),
        addr: addr_b,
        pubkey: id_b.public_key_base64().to_string(),
        source: crate::peer_table::PeerSource::Static,
        status: crate::peer_table::ConnectionStatus::Discovered,
        rtt_ms: None,
        last_seen: std::time::Instant::now(),
    };

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let conn = transport_a
        .ensure_connection(&peer_b)
        .await
        .expect("connect");
    assert!(conn.close_reason().is_none());
    assert!(transport_a.has_connection(id_b.agent_id()).await);
}

#[tokio::test]
async fn send_notify_unidirectional() {
    let dir_a = tempdir().expect("tempdir a");
    let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
    let id_a = Identity::load_or_generate(&paths_a).expect("identity a");

    let dir_b = tempdir().expect("tempdir b");
    let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
    let id_b = Identity::load_or_generate(&paths_b).expect("identity b");

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .expect("bind b");
    let addr_b = transport_b.local_addr().expect("local_addr b");
    let mut rx_b = transport_b.subscribe_inbound();

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .expect("bind a");

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = PeerRecord {
        agent_id: id_b.agent_id().to_string(),
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
        MessageKind::Notify,
        json!({"topic": "test", "data": {"msg": "hello"}, "importance": "low"}),
    );

    let result = transport_a
        .send(&peer_b, notify.clone())
        .await
        .expect("send");
    assert!(result.is_none());

    // Drain until we find the notify (hello is also broadcast)
    let received = loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
            .await
            .expect("timeout waiting for inbound")
            .expect("recv");
        if msg.kind != MessageKind::Hello {
            break msg;
        }
    };
    assert_eq!(received.kind, MessageKind::Notify);
    assert_eq!(received.from, id_a.agent_id());
}

#[tokio::test]
async fn send_ping_bidirectional() {
    let dir_a = tempdir().expect("tempdir a");
    let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
    let id_a = Identity::load_or_generate(&paths_a).expect("identity a");

    let dir_b = tempdir().expect("tempdir b");
    let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
    let id_b = Identity::load_or_generate(&paths_b).expect("identity b");

    let transport_b = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_b, 128)
        .await
        .expect("bind b");
    let addr_b = transport_b.local_addr().expect("local_addr b");

    let transport_a = QuicTransport::bind("127.0.0.1:0".parse().unwrap(), &id_a, 128)
        .await
        .expect("bind a");

    transport_a.set_expected_peer(
        id_b.agent_id().to_string(),
        id_b.public_key_base64().to_string(),
    );
    transport_b.set_expected_peer(
        id_a.agent_id().to_string(),
        id_a.public_key_base64().to_string(),
    );

    let peer_b = PeerRecord {
        agent_id: id_b.agent_id().to_string(),
        addr: addr_b,
        pubkey: id_b.public_key_base64().to_string(),
        source: crate::peer_table::PeerSource::Static,
        status: crate::peer_table::ConnectionStatus::Discovered,
        rtt_ms: None,
        last_seen: std::time::Instant::now(),
    };

    let ping = Envelope::new(
        id_a.agent_id().to_string(),
        id_b.agent_id().to_string(),
        MessageKind::Ping,
        json!({}),
    );

    let result = transport_a.send(&peer_b, ping.clone()).await.expect("send");
    let response = result.expect("expected response");
    assert_eq!(response.kind, MessageKind::Pong);
    assert_eq!(response.ref_id, Some(ping.id));
}
