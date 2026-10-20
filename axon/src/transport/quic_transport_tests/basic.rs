use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use tempfile::tempdir;

use super::fixtures::make_transport_pair;
use crate::config::AxonPaths;
use crate::identity::Identity;
use crate::message::{AgentId, Envelope, MessageKind};
use crate::peer_directory::{PeerDirectory, PeerStore};
use crate::transport::ConnectionManager;

#[tokio::test]
async fn endpoint_binds_and_reports_addr() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("identity");
    let directory = PeerDirectory::load(
        AgentId::parse(identity.agent_id()).unwrap(),
        PeerStore::new(paths.peers),
    )
    .await
    .unwrap();
    let transport =
        ConnectionManager::bind("127.0.0.1:0".parse().unwrap(), &identity, 128, directory)
            .await
            .expect("bind");
    assert_eq!(
        transport.local_addr().unwrap().ip(),
        "127.0.0.1".parse::<std::net::IpAddr>().unwrap()
    );
}

#[tokio::test]
async fn two_peers_connect() {
    let pair = make_transport_pair().await;
    let peer_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let envelope = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::Message,
        json!({"probe": true}),
    );
    pair.transport_a
        .send_to(&pair.directory_a, &peer_b, envelope, Duration::from_secs(5))
        .await
        .expect("connect and send");
    assert!(pair.transport_a.has_connection(&peer_b).await);
}

#[tokio::test]
async fn send_notify_unidirectional() {
    let pair = make_transport_pair().await;
    let mut rx_b = pair.transport_b.subscribe_inbound();
    let peer_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let notify = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::Message,
        json!({"topic": "test"}),
    );
    let result = pair
        .transport_a
        .send_to(&pair.directory_a, &peer_b, notify, Duration::from_secs(5))
        .await
        .expect("send");
    assert!(result.is_none());
    let received = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
        .await
        .expect("timeout")
        .expect("recv");
    assert_eq!(received.kind, MessageKind::Message);
    assert_eq!(received.from.as_deref(), Some(pair.id_a.agent_id()));
}

#[tokio::test]
async fn send_unknown_unidirectional_kind_preserves_name() {
    let pair = make_transport_pair().await;
    let mut rx_b = pair.transport_b.subscribe_inbound();
    let peer_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let future_kind = "future_kind_v99";
    let message = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::unknown(future_kind),
        json!({"future": true}),
    );
    let message_id = message.id;

    let result = pair
        .transport_a
        .send_to(&pair.directory_a, &peer_b, message, Duration::from_secs(5))
        .await
        .expect("send");
    assert!(result.is_none());

    let received = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = rx_b.recv().await.expect("recv");
            if message.id == message_id {
                break message;
            }
        }
    })
    .await
    .expect("timeout");
    assert_eq!(received.kind, MessageKind::unknown(future_kind));
    assert_eq!(received.kind.as_str(), future_kind);
}

/// I16 publisher half: broadcast eviction must be observable as a gap, not
/// silently dropped. A counted subscription folds `RecvError::Lagged` into
/// the manager's gap counter while preserving the raw receiver contract.
#[tokio::test]
async fn counted_subscription_records_broadcast_gaps() {
    use crate::transport::InboundReceiver;

    let pair = make_transport_pair().await;
    let mut rx: InboundReceiver = pair.transport_b.subscribe_inbound_counted();
    let envelope = std::sync::Arc::new(Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::Message,
        json!({"seq": 0}),
    ));

    // Fill the broadcast ring and then some while the subscriber never
    // polls: the oldest envelopes are evicted and must be reported.
    for _ in 0..600 {
        pair.transport_b
            .inbound_tx
            .send(envelope.clone())
            .expect("counted receiver is subscribed");
    }

    let err = rx
        .recv()
        .await
        .expect_err("lagging receiver must observe a gap");
    let tokio::sync::broadcast::error::RecvError::Lagged(missed) = err else {
        panic!("expected Lagged, got {err:?}");
    };
    assert!(missed > 0, "the ring must have evicted envelopes");
    assert_eq!(
        pair.transport_b.inbound_gap_count(),
        missed,
        "the gap counter must match the reported lag"
    );
    assert!(
        rx.recv().await.is_ok(),
        "surviving envelopes remain readable"
    );
}
