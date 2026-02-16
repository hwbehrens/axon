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
        .broadcast_inbound(&make_envelope(MessageKind::Request))
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
            .broadcast_inbound(&make_envelope(MessageKind::Request))
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
        .broadcast_inbound(&make_envelope(MessageKind::Request))
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
// §3.3 kinds filter + ack safety
// =========================================================================

/// A6: kinds filter stops at non-matching message to protect ack cursor.
#[tokio::test]
async fn kinds_filter_stops_at_non_matching_kind() {
    let server = bind_server(test_config()).await;

    // Buffer interleaved kinds: Request, Message, Request
    server
        .broadcast_inbound(&make_envelope(MessageKind::Request))
        .await
        .unwrap();
    server
        .broadcast_inbound(&make_envelope(MessageKind::Message))
        .await
        .unwrap();
    server
        .broadcast_inbound(&make_envelope(MessageKind::Request))
        .await
        .unwrap();

    hello_and_auth(&server, 1).await;

    // Fetch with kinds=[request] — should stop at seq=2 (Message), return only seq=1
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: Some(vec!["request".into()]),
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    let json = serde_json::to_value(&reply).unwrap();
    assert_ok(&reply);
    let msgs = json["messages"].as_array().unwrap();
    assert_eq!(
        msgs.len(),
        1,
        "should return only 1 contiguous matching msg"
    );
    assert_eq!(msgs[0]["seq"], 1);
    assert!(json["has_more"].as_bool().unwrap(), "has_more must be true");
}

/// A6: acking after kinds-filtered fetch does not skip non-matching messages.
#[tokio::test]
async fn ack_after_kinds_filter_does_not_skip_other_kinds() {
    let server = bind_server(test_config()).await;

    // Buffer: Request(1), Message(2), Request(3)
    server
        .broadcast_inbound(&make_envelope(MessageKind::Request))
        .await
        .unwrap();
    server
        .broadcast_inbound(&make_envelope(MessageKind::Message))
        .await
        .unwrap();
    server
        .broadcast_inbound(&make_envelope(MessageKind::Request))
        .await
        .unwrap();

    hello_and_auth(&server, 1).await;

    // Fetch with kinds=[request] — gets seq=1 only
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 50,
                kinds: Some(vec!["request".into()]),
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&reply).unwrap();
    let seq = json["next_seq"].as_u64().unwrap();

    // Ack seq=1
    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Ack {
                up_to_seq: seq,
                req_id: Some("r2".into()),
            },
        })
        .await
        .unwrap();
    assert_ok(&reply);

    // Fetch without filter — should see seq=2 (Message) and seq=3 (Request)
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
    let msgs = json["messages"].as_array().unwrap();
    assert_eq!(
        msgs.len(),
        2,
        "Message at seq=2 must not have been skipped by ack"
    );
    assert_eq!(msgs[0]["seq"], 2);
    assert_eq!(msgs[1]["seq"], 3);
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
            .broadcast_inbound(&make_envelope(MessageKind::Message))
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

// =========================================================================
// §3.3 Inbox limit boundary validation
// =========================================================================

/// IPC.md §3.3: inbox limit=0 is out of range (1–1000) → invalid_command.
#[tokio::test]
async fn inbox_limit_zero_rejected() {
    let server = bind_server(test_config()).await;
    hello_and_auth(&server, 1).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 0,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::InvalidCommand);
}

/// IPC.md §3.3: inbox limit=1001 is out of range (1–1000) → invalid_command.
#[tokio::test]
async fn inbox_limit_over_max_rejected() {
    let server = bind_server(test_config()).await;
    hello_and_auth(&server, 1).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 1001,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    assert_error(&reply, IpcErrorCode::InvalidCommand);
}

/// IPC.md §3.3: inbox limit=1 (min valid) succeeds.
#[tokio::test]
async fn inbox_limit_min_valid() {
    let server = bind_server(test_config()).await;
    hello_and_auth(&server, 1).await;

    // Buffer 3 messages
    for _ in 0..3 {
        server
            .broadcast_inbound(&make_envelope(MessageKind::Message))
            .await
            .unwrap();
    }

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 1,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    let json = serde_json::to_value(&reply).unwrap();
    assert_ok(&reply);
    assert_eq!(json["messages"].as_array().unwrap().len(), 1);
    assert!(json["has_more"].as_bool().unwrap());
}

/// IPC.md §3.3: inbox limit=1000 (max valid) succeeds.
#[tokio::test]
async fn inbox_limit_max_valid() {
    let server = bind_server(test_config()).await;
    hello_and_auth(&server, 1).await;

    let reply = server
        .handle_command(CommandEvent {
            client_id: 1,
            command: IpcCommand::Inbox {
                limit: 1000,
                kinds: None,
                req_id: Some("r1".into()),
            },
        })
        .await
        .unwrap();

    assert_ok(&reply);
}
