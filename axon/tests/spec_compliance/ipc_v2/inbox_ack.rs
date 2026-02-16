//! §3.3/§3.4 Inbox/ack cursor semantics and §4.3 multi-consumer independence.

use super::*;

// =========================================================================
// §3.3/§3.4 Inbox/ack cursor semantics
// =========================================================================

/// IPC.md §3.3: inbox messages include seq and buffered_at_ms.
#[tokio::test]
async fn inbox_returns_seq_and_buffered_at_ms() {
    let server = bind_server(test_config()).await;

    server
        .broadcast_inbound(&make_envelope(MessageKind::Query))
        .await
        .unwrap();

    hello_and_auth(&server, 1).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    let json = serde_json::to_value(&reply).unwrap();
    assert_ok(&reply);
    let msg = &json["messages"][0];
    assert!(msg["seq"].is_u64(), "seq must be present and numeric");
    assert!(
        msg["buffered_at_ms"].is_u64(),
        "buffered_at_ms must be present"
    );
    assert!(msg["envelope"].is_object(), "envelope must be present");
}

/// IPC.md §3.4: ack advances cursor; subsequent inbox shows fewer messages.
#[tokio::test]
async fn ack_advances_cursor() {
    let server = bind_server(test_config()).await;

    // Buffer 3 messages
    for _ in 0..3 {
        server
            .broadcast_inbound(&make_envelope(MessageKind::Query))
            .await
            .unwrap();
    }

    hello_and_auth(&server, 1).await;

    // Fetch all
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    let msgs = json["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);

    // Ack first two (seq of second message)
    let seq_2 = msgs[1]["seq"].as_u64().unwrap();
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Ack {
                up_to_seq: seq_2,
                req_id: Some("r2".into()),
            },
        })
        .await
        .unwrap();
    assert_ok(&reply);

    // Inbox should now show only 1 message
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r3".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["messages"].as_array().unwrap().len(), 1);
}

/// IPC.md §3.4: ack beyond delivered → ack_out_of_range.
#[tokio::test]
async fn ack_out_of_range_rejected() {
    let server = bind_server(test_config()).await;

    server
        .broadcast_inbound(&make_envelope(MessageKind::Query))
        .await
        .unwrap();

    hello_and_auth(&server, 1).await;

    // Fetch inbox to establish delivered seq
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    let max_seq = json["next_seq"].as_u64().unwrap();

    // Ack beyond delivered
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Ack {
                up_to_seq: max_seq + 100,
                req_id: Some("r2".into()),
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::AckOutOfRange);
}

// =========================================================================
// §4.3 Multi-consumer cursor independence
// =========================================================================

/// IPC.md §4.3: two consumers ack independently.
#[tokio::test]
async fn multi_consumer_cursor_independence() {
    let server = bind_server(test_config()).await;

    // Buffer 3 messages
    for _ in 0..3 {
        server
            .broadcast_inbound(&make_envelope(MessageKind::Notify))
            .await
            .unwrap();
    }

    // Consumer A (client 1)
    hello_and_auth_consumer(&server, 1, "consumer-a").await;
    // Consumer B (client 2)
    hello_and_auth_consumer(&server, 2, "consumer-b").await;

    // Consumer A: fetch inbox, ack all
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    let max_seq = json["next_seq"].as_u64().unwrap();
    server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Ack {
                up_to_seq: max_seq,
                req_id: Some("r2".into()),
            },
        })
        .await
        .unwrap();

    // Consumer A inbox should now be empty
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r3".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["messages"].as_array().unwrap().len(), 0);

    // Consumer B should still see all 3
    let reply = server
        .handle_command(CommandEvent {
            client_id: 2,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: None,
                req_id: Some("r4".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["messages"].as_array().unwrap().len(), 3);
}
