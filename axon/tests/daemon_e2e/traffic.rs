//! Full-stack IPC broadcast and request-scoping behavior across a real
//! daemon pair (IPC ↔ daemon ↔ QUIC).
//!
//! Restores the multi-daemon fanout coverage that the peer-directory
//! redesign removed, pinned to the DEC-013 semantics: ordinary inbound
//! messages broadcast to every connected IPC client; request responses go
//! only to the requesting client.

use super::*;

/// An inbound message is delivered to every connected IPC client on the
/// receiving daemon while each client keeps up with delivery.
#[tokio::test]
async fn message_broadcast_reaches_every_receiver_ipc_client() {
    let (daemon_a, daemon_b) = prepare_pair().await;

    let mut readers = Vec::new();
    let mut _keep_writes = Vec::new();
    for _ in 0..3 {
        let stream = UnixStream::connect(&daemon_b.paths.socket).await.unwrap();
        let (read, write) = stream.into_split();
        readers.push(BufReader::new(read));
        _keep_writes.push(write);
    }
    // Let every client register with the accept loop before sending.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let ack = ipc_command(
        &daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": daemon_b.identity.agent_id(),
            "kind": "message",
            "payload": {"topic": "fanout.test", "data": {"n": 1}}
        }),
    )
    .await;
    assert_eq!(ack["ok"], json!(true), "send must be accepted");

    for (index, reader) in readers.iter_mut().enumerate() {
        let inbound = read_json(reader).await;
        assert_eq!(
            inbound["event"], "inbound",
            "client {index} must receive the broadcast"
        );
        assert_eq!(inbound["envelope"]["kind"], "message");
        assert_eq!(inbound["envelope"]["payload"]["data"]["n"], 1);
    }

    daemon_a.stop().await;
    daemon_b.stop().await;
}

/// A request response is returned synchronously to the requesting IPC
/// client alone; other connected clients observe nothing (DEC-013: no
/// response broadcasting).
#[tokio::test]
async fn request_response_is_scoped_to_the_requesting_client_only() {
    let (daemon_a, daemon_b) = prepare_pair().await;

    // Two clients on A: one sends the request, one only listens.
    let sender_stream = UnixStream::connect(&daemon_a.paths.socket).await.unwrap();
    let (sender_read, mut sender_write) = sender_stream.into_split();
    let mut sender_reader = BufReader::new(sender_read);

    let listener_stream = UnixStream::connect(&daemon_a.paths.socket).await.unwrap();
    let (listener_read, _listener_write) = listener_stream.into_split();
    let mut listener_reader = BufReader::new(listener_read);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // No `serve` handler on B, so B answers with an immediate unhandled
    // error carried back inside the sender's send result.
    sender_write
        .write_all(
            format!(
                "{}\n",
                json!({
                    "cmd": "send",
                    "to": daemon_b.identity.agent_id(),
                    "kind": "request",
                    "payload": {},
                    "timeout_secs": 5
                })
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    let reply = read_json(&mut sender_reader).await;
    assert_eq!(reply["ok"], json!(true), "the send itself must succeed");
    assert_eq!(
        reply["response"]["kind"], "error",
        "B without a handler must answer with an error envelope"
    );
    assert_eq!(reply["response"]["payload"]["code"], "unhandled");

    // The listener must not receive the response as a broadcast.
    let mut stray = String::new();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        listener_reader.read_line(&mut stray),
    )
    .await;
    assert!(
        result.is_err(),
        "response leaked to non-requesting client: {stray}"
    );

    daemon_a.stop().await;
    daemon_b.stop().await;
}
