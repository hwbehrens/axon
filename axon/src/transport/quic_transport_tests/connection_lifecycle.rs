use super::fixtures::{make_transport_pair, peer_record, wait_for_registered_connection};
use crate::message::{Envelope, MessageKind};
use crate::transport::connection::run_connection;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, broadcast};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn simultaneous_dial_both_sides_can_message() {
    let pair = make_transport_pair().await;
    let addr_a = pair.transport_a.local_addr().expect("local_addr a");
    let addr_b = pair.transport_b.local_addr().expect("local_addr b");
    let peer_b = peer_record(&pair.id_b, addr_b);
    let peer_a = peer_record(&pair.id_a, addr_a);

    let (conn_a, conn_b) = tokio::join!(
        pair.transport_a.ensure_connection(&peer_b),
        pair.transport_b.ensure_connection(&peer_a),
    );
    conn_a.expect("A→B connect should succeed");
    conn_b.expect("B→A connect should succeed");

    assert!(pair.transport_a.has_connection(pair.id_b.agent_id()).await);
    assert!(pair.transport_b.has_connection(pair.id_a.agent_id()).await);

    let mut rx_b = pair.transport_b.subscribe_inbound();
    let notify = Envelope::new(
        pair.id_a.agent_id().to_string(),
        pair.id_b.agent_id().to_string(),
        MessageKind::Message,
        json!({"test": "simultaneous"}),
    );
    pair.transport_a
        .send(&peer_b, notify)
        .await
        .expect("send A→B should work after simultaneous dial");

    let received = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
        .await
        .expect("timeout")
        .expect("recv");
    assert_eq!(received.kind, MessageKind::Message);
}

#[tokio::test]
async fn ensure_connection_idempotent_after_simultaneous_dial() {
    let pair = make_transport_pair().await;
    let addr_a = pair.transport_a.local_addr().expect("local_addr a");
    let addr_b = pair.transport_b.local_addr().expect("local_addr b");
    let peer_b = peer_record(&pair.id_b, addr_b);
    let peer_a = peer_record(&pair.id_a, addr_a);

    let (_, _) = tokio::join!(
        pair.transport_a.ensure_connection(&peer_b),
        pair.transport_b.ensure_connection(&peer_a),
    );

    let conn1 = pair
        .transport_a
        .ensure_connection(&peer_b)
        .await
        .expect("re-ensure should succeed");
    let conn2 = pair
        .transport_a
        .ensure_connection(&peer_b)
        .await
        .expect("re-ensure should succeed again");
    assert_eq!(
        conn1.stable_id(),
        conn2.stable_id(),
        "repeated ensure_connection should return the same connection"
    );
}

#[tokio::test]
async fn superseded_connection_shutdown_preserves_newer_entry() {
    let pair = make_transport_pair().await;
    let addr_b = pair.transport_b.local_addr().expect("local_addr b");

    let conn1 = pair
        .transport_a
        .endpoint
        .connect(addr_b, pair.id_b.agent_id())
        .expect("begin outbound connect 1")
        .await
        .expect("complete outbound connect 1");

    let conn2 = pair
        .transport_a
        .endpoint
        .connect(addr_b, pair.id_b.agent_id())
        .expect("begin outbound connect 2")
        .await
        .expect("complete outbound connect 2");

    let shared_connections = Arc::new(RwLock::new(HashMap::new()));
    let (inbound_tx, _inbound_rx) = broadcast::channel(16);
    let cancel1 = CancellationToken::new();
    let cancel2 = CancellationToken::new();

    let task1 = tokio::spawn(run_connection(
        conn1.clone(),
        pair.id_a.agent_id().to_string(),
        inbound_tx.clone(),
        shared_connections.clone(),
        cancel1.clone(),
        None,
        Duration::from_secs(10),
        None,
    ));
    wait_for_registered_connection(&shared_connections, pair.id_b.agent_id(), conn1.stable_id())
        .await;

    let task2 = tokio::spawn(run_connection(
        conn2.clone(),
        pair.id_a.agent_id().to_string(),
        inbound_tx,
        shared_connections.clone(),
        cancel2.clone(),
        None,
        Duration::from_secs(10),
        None,
    ));
    wait_for_registered_connection(&shared_connections, pair.id_b.agent_id(), conn2.stable_id())
        .await;

    cancel1.cancel();
    tokio::time::timeout(Duration::from_secs(5), task1)
        .await
        .expect("task1 join timeout")
        .expect("task1 should exit cleanly");

    let current_stable_id = shared_connections
        .read()
        .await
        .get(pair.id_b.agent_id())
        .map(|c| c.stable_id());
    assert_eq!(
        current_stable_id,
        Some(conn2.stable_id()),
        "superseded loop must not remove newer connection entry"
    );

    cancel2.cancel();
    tokio::time::timeout(Duration::from_secs(5), task2)
        .await
        .expect("task2 join timeout")
        .expect("task2 should exit cleanly");
}
