use super::super::*;
use super::{connect_v2, ipc_v2_command, wait_for_buffered_messages};

/// IPC v2 subscribe with replay: messages buffered before subscribe
/// are replayed as events with replay=true.
#[tokio::test]
async fn v2_subscribe_with_replay_e2e() {
    let td = setup_connected_pair().await;

    // Send two messages from A → B before subscribing
    for topic in &["replay.1", "replay.2"] {
        let ack = ipc_command(
            &td.daemon_a.paths.socket,
            json!({
                "cmd": "send",
                "to": td.id_b.agent_id(),
                "kind": "notify",
                "payload": {"topic": topic, "data": {}, "importance": "low"}
            }),
        )
        .await
        .unwrap();
        assert_eq!(ack["ok"], json!(true));
    }

    // Wait for messages to arrive at B
    wait_for_buffered_messages(&td.daemon_b.paths.socket, "notify", 2).await;

    // Connect v2 client to B and subscribe with replay.
    // We send the subscribe command manually (not via ipc_v2_command) because
    // replayed events are interleaved before the subscribe reply and we need
    // to capture them.
    let (mut writer_b, mut reader_b) = connect_v2(&td.daemon_b.paths.socket, "default").await;

    let sub_cmd = json!({"cmd": "subscribe", "replay": true, "req_id": "s1"});
    let line_out = serde_json::to_string(&sub_cmd).unwrap();
    writer_b.write_all(line_out.as_bytes()).await.unwrap();
    writer_b.write_all(b"\n").await.unwrap();

    // Read all replay events + the subscribe reply
    let mut notify_replays = Vec::new();
    let mut sub_reply: Option<Value> = None;

    for _ in 0..20 {
        let mut line = String::new();
        let read_result = timeout(Duration::from_millis(2000), reader_b.read_line(&mut line)).await;
        if read_result.is_err() {
            break;
        }
        if line.trim().is_empty() {
            break;
        }
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        if v.get("event").is_some() && v["replay"] == true {
            if v["envelope"]["kind"] == "notify" {
                notify_replays.push(v);
            }
        } else if v.get("subscribed").is_some() {
            sub_reply = Some(v);
            // Subscribe reply comes last; stop reading
            break;
        }
    }

    let sub_reply = sub_reply.expect("should receive subscribe reply");
    assert_eq!(sub_reply["ok"], true);
    assert_eq!(sub_reply["subscribed"], true);
    assert_eq!(sub_reply["req_id"], "s1");
    let replayed = sub_reply["replayed"].as_u64().unwrap();
    assert!(
        replayed >= 2,
        "should replay at least 2 messages, got {replayed}"
    );

    assert_eq!(
        notify_replays.len(),
        2,
        "should replay exactly 2 notify messages"
    );
    assert_eq!(notify_replays[0]["envelope"]["from"], td.id_a.agent_id());
    assert_eq!(notify_replays[1]["envelope"]["from"], td.id_a.agent_id());

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}

/// IPC v2 kind-filtered subscribe: only subscribed kinds are delivered.
#[tokio::test]
async fn v2_subscribe_kind_filter_e2e() {
    let td = setup_connected_pair().await;

    // Connect v2 client to B and subscribe to only "notify" kinds
    let (mut writer_b, mut reader_b) = connect_v2(&td.daemon_b.paths.socket, "default").await;

    let sub_reply = ipc_v2_command(
        &mut writer_b,
        &mut reader_b,
        json!({"cmd": "subscribe", "replay": false, "kinds": ["notify"], "req_id": "s1"}),
    )
    .await;
    assert_eq!(sub_reply["ok"], true);
    assert_eq!(sub_reply["subscribed"], true);

    // Send a query from A → B (should NOT be delivered to subscriber)
    let ack1 = ipc_command(
        &td.daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": td.id_b.agent_id(),
            "kind": "query",
            "payload": {"question": "filtered?", "domain": "test"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack1["ok"], json!(true));

    // Send a notify from A → B (SHOULD be delivered)
    let ack2 = ipc_command(
        &td.daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": td.id_b.agent_id(),
            "kind": "notify",
            "payload": {"topic": "filtered.test", "data": {}, "importance": "low"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack2["ok"], json!(true));

    // Read events from B — should only get the notify, not the query
    let mut received_events = Vec::new();
    for _ in 0..5 {
        let mut line = String::new();
        let read_result = timeout(Duration::from_millis(2000), reader_b.read_line(&mut line)).await;
        if read_result.is_err() {
            break;
        }
        if line.trim().is_empty() {
            break;
        }
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        if v.get("event").is_some() {
            received_events.push(v);
        }
    }

    // We should have received at least the notify
    let notify_events: Vec<&Value> = received_events
        .iter()
        .filter(|e| e["envelope"]["kind"] == "notify")
        .collect();
    let query_events: Vec<&Value> = received_events
        .iter()
        .filter(|e| e["envelope"]["kind"] == "query")
        .collect();

    assert!(!notify_events.is_empty(), "should receive notify events");
    assert!(
        query_events.is_empty(),
        "should NOT receive query events (filtered out)"
    );

    td.daemon_a.shutdown().await;
    td.daemon_b.shutdown().await;
}
