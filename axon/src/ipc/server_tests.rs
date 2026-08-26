use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use serde_json::json;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::*;
use crate::message::MessageKind;

fn test_server_with_clients(clients: HashMap<u64, mpsc::Sender<Arc<str>>>) -> IpcServer {
    let clients = clients
        .into_iter()
        .map(|(id, tx)| {
            (
                id,
                ClientHandle {
                    tx,
                    cancel: CancellationToken::new(),
                },
            )
        })
        .collect();
    IpcServer {
        socket_path: PathBuf::from("/tmp/axon-test.sock"),
        max_clients: 64,
        clients: Arc::new(Mutex::new(clients)),
        next_client_id: Arc::new(AtomicU64::new(1)),
        owner_uid: 0,
        max_client_queue: 8,
        config: Arc::new(IpcServerConfig::default()),
        disconnected_tx: broadcast::channel(8).0,
        cancel: CancellationToken::new(),
        tasks: TaskTracker::new(),
    }
}

#[tokio::test]
async fn broadcast_inbound_disconnects_full_client_queue() {
    let (slow_tx, mut slow_rx) = mpsc::channel::<Arc<str>>(1);
    slow_tx
        .try_send(Arc::from("{\"prefill\":true}"))
        .expect("prefill slow queue");

    let (healthy_tx, mut healthy_rx) = mpsc::channel::<Arc<str>>(8);

    let mut clients = HashMap::new();
    clients.insert(1, slow_tx);
    clients.insert(2, healthy_tx);
    let server = test_server_with_clients(clients);

    let envelope = Envelope::new(
        crate::message::AgentId::parse("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
        crate::message::AgentId::parse("ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
        MessageKind::Message,
        json!({"data": "x"}),
    );
    server
        .broadcast_inbound(&envelope)
        .await
        .expect("broadcast");

    assert_eq!(
        server.client_count().await,
        1,
        "lagging client should be disconnected"
    );
    let received = healthy_rx
        .recv()
        .await
        .expect("healthy client should receive");
    assert!(received.contains("\"event\":\"inbound\""));
    assert!(
        slow_rx.try_recv().is_ok(),
        "slow queue keeps only prefilled data"
    );
}

#[tokio::test]
async fn close_client_cancels_client_handle() {
    let (tx, _rx) = mpsc::channel::<Arc<str>>(1);
    let cancel = CancellationToken::new();

    let mut clients = HashMap::new();
    clients.insert(
        7,
        ClientHandle {
            tx,
            cancel: cancel.clone(),
        },
    );
    let server = IpcServer {
        socket_path: PathBuf::from("/tmp/axon-test.sock"),
        max_clients: 64,
        clients: Arc::new(Mutex::new(clients)),
        next_client_id: Arc::new(AtomicU64::new(1)),
        owner_uid: 0,
        max_client_queue: 8,
        config: Arc::new(IpcServerConfig::default()),
        disconnected_tx: broadcast::channel(8).0,
        cancel: CancellationToken::new(),
        tasks: TaskTracker::new(),
    };

    server.close_client(7).await;

    assert_eq!(server.client_count().await, 0);
    assert!(
        cancel.is_cancelled(),
        "close_client should signal cancellation for active client handler"
    );
}

#[tokio::test]
async fn broadcast_peer_candidate_reaches_connected_clients() {
    let (tx_a, mut rx_a) = mpsc::channel::<Arc<str>>(8);
    let (tx_b, mut rx_b) = mpsc::channel::<Arc<str>>(8);

    let mut clients = HashMap::new();
    clients.insert(1, tx_a);
    clients.insert(2, tx_b);
    let server = test_server_with_clients(clients);

    server
        .broadcast_peer_candidate(
            "ed25519.cccccccccccccccccccccccccccccccc",
            "cHVia2V5",
            vec!["127.0.0.1:7100".to_string()],
            "handshake",
        )
        .await
        .expect("pair request broadcast");

    let line_a = rx_a.recv().await.expect("client A event");
    let line_b = rx_b.recv().await.expect("client B event");

    assert!(line_a.contains("\"event\":\"peer_candidate\""));
    assert!(line_a.contains("\"public_key\":\"cHVia2V5\""));
    assert!(line_b.contains("\"event\":\"peer_candidate\""));
    assert!(line_b.contains("\"agent_id\":\"ed25519.cccccccccccccccccccccccccccccccc\""));
}

// ---------------------------------------------------------------------------
// Round-seven review pins (DEC-022): every outbound line passes one encoder
// enforcing the framed limit (newline included). Oversized broadcasts are
// dropped with a warning, never truncated; oversized replies fail
// explicitly with `message_too_large`; oversized handler deliveries fail so
// the broker can send the remote requester a terminal error.
// ---------------------------------------------------------------------------

fn agent_a() -> crate::message::AgentId {
    crate::message::AgentId::parse("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()
}

fn agent_b() -> crate::message::AgentId {
    crate::message::AgentId::parse("ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap()
}

fn single_client_server() -> (IpcServer, mpsc::Receiver<Arc<str>>) {
    let (tx, rx) = mpsc::channel::<Arc<str>>(8);
    let mut clients = HashMap::new();
    clients.insert(1u64, tx);
    (test_server_with_clients(clients), rx)
}

#[tokio::test]
async fn oversized_inbound_event_is_dropped_never_truncated() {
    let (server, mut rx) = single_client_server();

    let huge = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"pad": "x".repeat(70_000)}),
    );
    server
        .broadcast_inbound(&huge)
        .await
        .expect("an oversized broadcast is a logged drop, not an error");
    assert!(
        rx.try_recv().is_err(),
        "no truncated or oversized event may be delivered"
    );

    // Healthy events still flow afterwards.
    let small = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"data": 1}),
    );
    server
        .broadcast_inbound(&small)
        .await
        .expect("small broadcast");
    let line = rx.recv().await.expect("small event delivered");
    assert!(line.len() < crate::ipc::MAX_IPC_LINE_LENGTH);
}

#[tokio::test]
async fn oversized_reply_fails_explicitly_with_message_too_large() {
    let (server, mut rx) = single_client_server();

    let huge = DaemonReply::SendOk {
        ok: true,
        msg_id: uuid::Uuid::new_v4(),
        req_id: Some("req-7".to_string()),
        response: Some(Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Response,
            json!({"pad": "x".repeat(70_000)}),
        )),
    };
    server
        .send_reply(1, &huge)
        .await
        .expect("oversized reply delivery itself succeeds");

    let line = rx
        .recv()
        .await
        .expect("explicit error reply replaces the payload");
    let decoded: serde_json::Value = serde_json::from_str(&line).expect("error reply is JSON");
    assert_eq!(decoded["ok"], json!(false));
    assert_eq!(decoded["error"], json!("message_too_large"));
    assert_eq!(
        decoded["req_id"],
        json!("req-7"),
        "correlation must survive"
    );
}

#[tokio::test]
async fn oversized_request_event_fails_delivery_explicitly() {
    let (server, mut rx) = single_client_server();

    let huge_request = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Request,
        json!({"pad": "x".repeat(70_000)}),
    );
    let result = server.send_request_event(1, &huge_request).await;
    assert!(
        result.is_err(),
        "oversized handler delivery must fail so the broker sends the \
         remote requester one terminal error response"
    );
    assert!(rx.try_recv().is_err(), "nothing may be delivered");
}
