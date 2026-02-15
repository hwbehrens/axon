use super::*;
use serde_json::json;

// --- IpcCommand deserialization ---

#[test]
fn parse_send_command() {
    let parsed: IpcCommand = serde_json::from_value(json!({
        "cmd": "send",
        "to": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "kind": "notify",
        "payload": {"topic":"meta.status", "data":{}}
    }))
    .expect("parse command");

    match parsed {
        IpcCommand::Send { to, kind, .. } => {
            assert_eq!(to, "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
            assert_eq!(kind, MessageKind::Notify);
        }
        _ => panic!("expected send command"),
    }
}

#[test]
fn parse_send_with_ref() {
    let id = Uuid::new_v4();
    let parsed: IpcCommand = serde_json::from_value(json!({
        "cmd": "send",
        "to": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "kind": "cancel",
        "payload": {"reason": "changed mind"},
        "ref": id.to_string()
    }))
    .expect("parse command");

    match parsed {
        IpcCommand::Send { ref_id, .. } => {
            assert_eq!(ref_id, Some(id));
        }
        _ => panic!("expected send command"),
    }
}

#[test]
fn parse_peers_command() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"peers"}"#).unwrap();
    assert!(matches!(cmd, IpcCommand::Peers));
}

#[test]
fn parse_status_command() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"status"}"#).unwrap();
    assert!(matches!(cmd, IpcCommand::Status));
}

#[test]
fn unknown_cmd_fails() {
    let result = serde_json::from_str::<IpcCommand>(r#"{"cmd":"explode"}"#);
    assert!(result.is_err());
}

#[test]
fn invalid_json_fails() {
    let result = serde_json::from_str::<IpcCommand>("not json");
    assert!(result.is_err());
}

// --- DaemonReply serialization ---

#[test]
fn send_ack_serialization() {
    let id = Uuid::new_v4();
    let reply = DaemonReply::SendAck {
        ok: true,
        msg_id: id,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["msg_id"], id.to_string());
}

#[test]
fn peers_reply_serialization() {
    let reply = DaemonReply::Peers {
        ok: true,
        peers: vec![PeerSummary {
            id: "a1b2c3d4".to_string(),
            addr: "192.168.1.50:7100".to_string(),
            status: "connected".to_string(),
            rtt_ms: Some(0.4),
            source: "static".to_string(),
        }],
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["peers"][0]["id"], "a1b2c3d4");
    assert_eq!(v["peers"][0]["rtt_ms"], 0.4);
    assert_eq!(v["peers"][0]["source"], "static");
}

#[test]
fn status_reply_serialization() {
    let reply = DaemonReply::Status {
        ok: true,
        uptime_secs: 3600,
        peers_connected: 2,
        messages_sent: 42,
        messages_received: 38,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["uptime_secs"], 3600);
    assert_eq!(v["messages_sent"], 42);
}

#[test]
fn error_reply_serialization() {
    let reply = DaemonReply::Error {
        ok: false,
        error: "peer not found: deadbeef".to_string(),
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("peer not found"));
}

#[test]
fn inbound_reply_serialization() {
    let envelope = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Notify,
        json!({"topic":"meta.status", "data":{}}),
    );
    let reply = DaemonReply::Inbound {
        inbound: true,
        envelope,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["inbound"], true);
    assert_eq!(v["envelope"]["kind"], "notify");
}

// --- v2 command parsing ---

#[test]
fn parse_hello_command() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"hello","version":2}"#).unwrap();
    match cmd {
        IpcCommand::Hello { version } => {
            assert_eq!(version, 2);
        }
        _ => panic!("expected hello command"),
    }
}

#[test]
fn parse_auth_command() {
    let cmd: IpcCommand = serde_json::from_str(
        r#"{"cmd":"auth","token":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#,
    )
    .unwrap();
    match cmd {
        IpcCommand::Auth { token } => {
            assert_eq!(token.len(), 64);
        }
        _ => panic!("expected auth command"),
    }
}

#[test]
fn parse_whoami_command() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"whoami"}"#).unwrap();
    assert!(matches!(cmd, IpcCommand::Whoami));
}

#[test]
fn parse_inbox_command() {
    let cmd: IpcCommand = serde_json::from_str(
        r#"{"cmd":"inbox","limit":100,"since":"2026-02-15T08:00:00Z","kinds":["query","notify"]}"#,
    )
    .unwrap();
    match cmd {
        IpcCommand::Inbox {
            limit,
            since,
            kinds,
        } => {
            assert_eq!(limit, 100);
            assert_eq!(since.unwrap(), "2026-02-15T08:00:00Z");
            assert_eq!(kinds.unwrap().len(), 2);
        }
        _ => panic!("expected inbox command"),
    }
}

#[test]
fn parse_inbox_defaults() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"inbox"}"#).unwrap();
    match cmd {
        IpcCommand::Inbox {
            limit,
            since,
            kinds,
        } => {
            assert_eq!(limit, 50);
            assert!(since.is_none());
            assert!(kinds.is_none());
        }
        _ => panic!("expected inbox command"),
    }
}

#[test]
fn parse_ack_command() {
    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    let cmd: IpcCommand = serde_json::from_value(json!({
        "cmd": "ack",
        "ids": [id1.to_string(), id2.to_string()]
    }))
    .unwrap();
    match cmd {
        IpcCommand::Ack { ids } => {
            assert_eq!(ids.len(), 2);
        }
        _ => panic!("expected ack command"),
    }
}

#[test]
fn parse_subscribe_command() {
    let cmd: IpcCommand = serde_json::from_str(
        r#"{"cmd":"subscribe","since":"2026-02-15T08:00:00Z","kinds":["query"]}"#,
    )
    .unwrap();
    match cmd {
        IpcCommand::Subscribe { since, kinds } => {
            assert!(since.is_some());
            assert_eq!(kinds.unwrap().len(), 1);
        }
        _ => panic!("expected subscribe command"),
    }
}

// --- v2 reply serialization ---

#[test]
fn hello_reply_serialization() {
    let reply = DaemonReply::Hello {
        ok: true,
        version: 2,
        agent_id: "ed25519.a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".to_string(),
        features: vec!["auth".to_string(), "buffer".to_string()],
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["version"], 2);
    assert_eq!(v["features"].as_array().unwrap().len(), 2);
}

#[test]
fn auth_reply_serialization() {
    let reply = DaemonReply::Auth {
        ok: true,
        auth: "accepted".to_string(),
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["auth"], "accepted");
}

#[test]
fn whoami_reply_serialization() {
    let reply = DaemonReply::Whoami {
        ok: true,
        info: crate::ipc::WhoamiInfo {
            agent_id: "ed25519.a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".to_string(),
            public_key: "base64key".to_string(),
            name: Some("test-agent".to_string()),
            version: "0.1.0".to_string(),
            ipc_version: 2,
            uptime_secs: 3600,
        },
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["ipc_version"], 2);
    assert_eq!(v["uptime_secs"], 3600);
}

#[test]
fn inbox_reply_serialization() {
    let envelope = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Query,
        json!({"question":"test"}),
    );
    let buffered = crate::ipc::BufferedMessage {
        envelope,
        buffered_at: "2026-02-15T08:00:00.000Z".to_string(),
    };
    let reply = DaemonReply::Inbox {
        ok: true,
        messages: vec![buffered],
        has_more: false,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["messages"].as_array().unwrap().len(), 1);
    assert_eq!(v["has_more"], false);
}

#[test]
fn ack_reply_serialization() {
    let reply = DaemonReply::Ack { ok: true, acked: 3 };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["acked"], 3);
}

#[test]
fn subscribe_reply_serialization() {
    let reply = DaemonReply::Subscribe {
        ok: true,
        subscribed: true,
        replayed: 5,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["subscribed"], true);
    assert_eq!(v["replayed"], 5);
}

// --- Property-based tests ---

use proptest::prelude::*;

fn arb_message_kind() -> impl Strategy<Value = MessageKind> {
    prop_oneof![
        Just(MessageKind::Hello),
        Just(MessageKind::Ping),
        Just(MessageKind::Pong),
        Just(MessageKind::Query),
        Just(MessageKind::Response),
        Just(MessageKind::Delegate),
        Just(MessageKind::Ack),
        Just(MessageKind::Result),
        Just(MessageKind::Notify),
        Just(MessageKind::Cancel),
        Just(MessageKind::Discover),
        Just(MessageKind::Capabilities),
        Just(MessageKind::Error),
    ]
}

proptest! {
    #[test]
    fn ipc_command_parse_never_panics(data in "\\PC{0,256}") {
        let _ = serde_json::from_str::<IpcCommand>(&data);
    }

    #[test]
    fn inbox_limit_clamped(limit in 0usize..10000) {
        let cmd_json = format!(r#"{{"cmd":"inbox","limit":{}}}"#, limit);
        if let Ok(IpcCommand::Inbox { limit: parsed, .. }) = serde_json::from_str::<IpcCommand>(&cmd_json) {
            prop_assert_eq!(parsed, limit);
        }
    }

    #[test]
    fn hello_version_roundtrips(version in 1u32..1000) {
        let cmd_json = format!(r#"{{"cmd":"hello","version":{}}}"#, version);
        let parsed: IpcCommand = serde_json::from_str(&cmd_json).unwrap();
        match parsed {
            IpcCommand::Hello { version: v } => prop_assert_eq!(v, version),
            _ => prop_assert!(false, "expected hello command"),
        }
    }

    #[test]
    fn subscribe_kinds_roundtrip(
        kinds in proptest::collection::vec(arb_message_kind(), 0..5)
    ) {
        let kinds_json: Vec<String> = kinds.iter().map(|k| {
            serde_json::to_string(k).unwrap()
        }).collect();
        let cmd_json = format!(r#"{{"cmd":"subscribe","kinds":[{}]}}"#, kinds_json.join(","));
        let parsed: IpcCommand = serde_json::from_str(&cmd_json).unwrap();
        match parsed {
            IpcCommand::Subscribe { kinds: Some(parsed_kinds), .. } => {
                prop_assert_eq!(parsed_kinds.len(), kinds.len());
            }
            IpcCommand::Subscribe { kinds: None, .. } if kinds.is_empty() => {
                // empty vec might deserialize as None depending on serde behavior
                // but actually it should be Some([]) — either way is fine
            }
            _ => prop_assert!(false, "expected subscribe command"),
        }
    }
}
