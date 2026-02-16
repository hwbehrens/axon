use super::*;

/// IPC.md §3.2: whoami response includes identity fields.
#[test]
fn ipc_v2_whoami_command_and_response_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_value(json!({
        "cmd": "whoami"
    }))
    .unwrap();
    assert!(matches!(cmd, axon::ipc::IpcCommand::Whoami { .. }));

    let reply = axon::ipc::DaemonReply::Whoami {
        ok: true,
        info: axon::ipc::WhoamiInfo {
            agent_id: "ed25519.test".to_string(),
            public_key: "pubkey_base64".to_string(),
            name: Some("test-agent".to_string()),
            version: "0.1.0".to_string(),
            ipc_version: 2,
            uptime_secs: 123,
        },
        req_id: None,
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["ok"], true);
    assert_eq!(j["agent_id"], "ed25519.test");
    assert_eq!(j["ipc_version"], 2);
    assert_eq!(j["uptime_secs"], 123);
}

/// IPC.md §3.3: inbox command with limit parameter.
#[test]
fn ipc_v2_inbox_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_value(json!({
        "cmd": "inbox",
        "limit": 10
    }))
    .unwrap();
    match cmd {
        axon::ipc::IpcCommand::Inbox { limit, kinds, .. } => {
            assert_eq!(limit, 10);
            assert!(kinds.is_none());
        }
        _ => panic!("expected Inbox"),
    }
}

/// IPC.md §3.3: inbox command with limit and kinds parameters.
#[test]
fn ipc_v2_inbox_command_with_filters() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_value(json!({
        "cmd": "inbox",
        "limit": 100,
        "kinds": ["query", "notify"]
    }))
    .unwrap();
    match cmd {
        axon::ipc::IpcCommand::Inbox { limit, kinds, .. } => {
            assert_eq!(limit, 100);
            let k = kinds.unwrap();
            assert_eq!(k.len(), 2);
            assert!(k.contains(&"query".to_string()));
            assert!(k.contains(&"notify".to_string()));
        }
        _ => panic!("expected Inbox"),
    }
}

/// IPC.md §3.3: inbox response includes messages array and has_more.
#[test]
fn ipc_v2_inbox_response_shape() {
    let reply = axon::ipc::DaemonReply::Inbox {
        ok: true,
        messages: vec![],
        next_seq: Some(1),
        has_more: false,
        req_id: None,
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["ok"], true);
    assert!(j["messages"].is_array());
    assert_eq!(j["has_more"], false);
}

/// IPC.md §3.4: ack command includes array of UUIDs.
#[test]
fn ipc_v2_ack_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_value(json!({
        "cmd": "ack",
        "up_to_seq": 102
    }))
    .unwrap();
    match cmd {
        axon::ipc::IpcCommand::Ack { up_to_seq, .. } => {
            assert_eq!(up_to_seq, 102);
        }
        _ => panic!("expected Ack"),
    }
}

/// IPC.md §3.4: ack response includes count of acknowledged messages.
#[test]
fn ipc_v2_ack_response_shape() {
    let reply = axon::ipc::DaemonReply::Ack {
        ok: true,
        acked_seq: 102,
        req_id: None,
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["ok"], true);
    assert_eq!(j["acked_seq"], 102);
}

/// IPC.md §3.5: subscribe command with optional since and kinds.
#[test]
fn ipc_v2_subscribe_command_shape() {
    let cmd: axon::ipc::IpcCommand = serde_json::from_value(json!({
        "cmd": "subscribe",
        "replay": true,
        "kinds": ["notify"]
    }))
    .unwrap();
    match cmd {
        axon::ipc::IpcCommand::Subscribe { replay, kinds, .. } => {
            assert!(replay);
            assert_eq!(kinds.unwrap().len(), 1);
        }
        _ => panic!("expected Subscribe"),
    }
}

/// IPC.md §3.5: subscribe response includes subscribed flag and replayed count.
#[test]
fn ipc_v2_subscribe_response_shape() {
    let reply = axon::ipc::DaemonReply::Subscribe {
        ok: true,
        subscribed: true,
        replayed: 3,
        replay_to_seq: 5,
        req_id: None,
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["ok"], true);
    assert_eq!(j["subscribed"], true);
    assert_eq!(j["replayed"], 3);
    assert_eq!(j["replay_to_seq"], 5);
}
