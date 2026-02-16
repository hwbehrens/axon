use super::super::*;
use super::{connect_v2, ipc_v2_command, wait_for_buffered_messages};

/// IPC v2 inbox/ack round-trip: message is delivered via QUIC, then
/// retrieved via inbox and acknowledged.
#[tokio::test]
async fn v2_inbox_and_ack_with_real_traffic() {
    let td = setup_connected_pair().await;

    // Send a notify from A → B before B's v2 client connects
    let ack = ipc_command(
        &td.daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": td.id_b.agent_id(),
            "kind": "message",
            "payload": {"topic": "buffered.test", "data": {}, "importance": "low"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack["ok"], json!(true));

    // Wait for the message to arrive at B
    wait_for_buffered_messages(&td.daemon_b.paths.socket, "message", 1).await;

    // Now connect a v2 client to B and fetch inbox
    let (mut writer_b, mut reader_b) = connect_v2(&td.daemon_b.paths.socket, "default").await;

    let inbox = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "inbox", "limit": 50, "req_id": "i1"}),
    )
    .await;

    assert_eq!(inbox["ok"], true);
    assert_eq!(inbox["req_id"], "i1");
    let messages = inbox["messages"].as_array().unwrap();
    assert!(
        !messages.is_empty(),
        "inbox should contain the buffered message"
    );

    // Find the message we sent
    let notify_msg = messages
        .iter()
        .find(|m| m["envelope"]["kind"] == "message")
        .expect("should find the message in inbox");
    assert_eq!(notify_msg["envelope"]["from"], td.id_a.agent_id());
    let seq = notify_msg["seq"].as_u64().unwrap();

    // Ack the message
    let ack_reply = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "ack", "up_to_seq": seq, "req_id": "a1"}),
    )
    .await;
    assert_eq!(ack_reply["ok"], true);
    assert_eq!(ack_reply["acked_seq"], seq);

    // Inbox should now be empty (for this consumer)
    let inbox2 = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "inbox", "limit": 50, "req_id": "i2"}),
    )
    .await;
    assert_eq!(inbox2["ok"], true);

    // Filter to just message kind to avoid counting hello/etc
    let remaining: Vec<&Value> = inbox2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["envelope"]["kind"] == "message")
        .collect();
    assert_eq!(remaining.len(), 0, "no messages should remain after ack");

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}

/// IPC v2 multi-consumer independence: two consumers have independent
/// cursors over the same daemon's receive buffer.
#[tokio::test]
async fn v2_multi_consumer_e2e() {
    let td = setup_connected_pair().await;

    // Send a message from A → B
    let ack = ipc_command(
        &td.daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": td.id_b.agent_id(),
            "kind": "message",
            "payload": {"topic": "multi.test", "data": {}, "importance": "low"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack["ok"], json!(true));

    wait_for_buffered_messages(&td.daemon_b.paths.socket, "message", 1).await;

    // Connect consumer A
    let (mut writer_a, mut reader_a) = connect_v2(&td.daemon_b.paths.socket, "consumer_a").await;

    // Connect consumer B
    let (mut writer_b, mut reader_b) = connect_v2(&td.daemon_b.paths.socket, "consumer_b").await;

    // Consumer A: fetch inbox
    let inbox_a = ipc_v2_command(
        &mut writer_a,
        &mut reader_a,
        json!({"cmd": "inbox", "limit": 50, "req_id": "ia1"}),
    )
    .await;
    assert_eq!(inbox_a["ok"], true);
    let msgs_a: Vec<&Value> = inbox_a["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["envelope"]["kind"] == "message")
        .collect();
    assert!(!msgs_a.is_empty(), "consumer A should see the message");

    // Consumer A: ack the highest seq
    let max_seq = inbox_a["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["seq"].as_u64().unwrap())
        .max()
        .unwrap();
    let ack_a = ipc_v2_command(
        &mut writer_a,
        &mut reader_a,
        json!({"cmd": "ack", "up_to_seq": max_seq, "req_id": "aa1"}),
    )
    .await;
    assert_eq!(ack_a["ok"], true);

    // Consumer B should STILL see messages (independent cursor)
    let inbox_b = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "inbox", "limit": 50, "req_id": "ib1"}),
    )
    .await;
    assert_eq!(inbox_b["ok"], true);
    let msgs_b: Vec<&Value> = inbox_b["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["envelope"]["kind"] == "message")
        .collect();
    assert!(
        !msgs_b.is_empty(),
        "consumer B should still see the message (independent cursor)"
    );

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}
