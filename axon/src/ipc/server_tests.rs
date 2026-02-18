use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use serde_json::json;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

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
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
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
    };

    server.close_client(7).await;

    assert_eq!(server.client_count().await, 0);
    assert!(
        cancel.is_cancelled(),
        "close_client should signal cancellation for active client handler"
    );
}

#[tokio::test]
async fn broadcast_pair_request_reaches_connected_clients() {
    let (tx_a, mut rx_a) = mpsc::channel::<Arc<str>>(8);
    let (tx_b, mut rx_b) = mpsc::channel::<Arc<str>>(8);

    let mut clients = HashMap::new();
    clients.insert(1, tx_a);
    clients.insert(2, tx_b);
    let server = test_server_with_clients(clients);

    server
        .broadcast_pair_request(
            "ed25519.cccccccccccccccccccccccccccccccc",
            "cHVia2V5",
            Some("127.0.0.1:7100"),
        )
        .await
        .expect("pair request broadcast");

    let line_a = rx_a.recv().await.expect("client A event");
    let line_b = rx_b.recv().await.expect("client B event");

    assert!(line_a.contains("\"event\":\"pair_request\""));
    assert!(line_a.contains("\"pubkey\":\"cHVia2V5\""));
    assert!(line_b.contains("\"event\":\"pair_request\""));
    assert!(line_b.contains("\"agent_id\":\"ed25519.cccccccccccccccccccccccccccccccc\""));
}
