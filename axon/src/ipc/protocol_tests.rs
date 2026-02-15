use super::*;
use serde_json::Value;
use serde_json::json;
use uuid::Uuid;

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
fn parse_send_with_req_id() {
    let parsed: IpcCommand = serde_json::from_value(json!({
        "cmd": "send",
        "to": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "kind": "notify",
        "payload": {},
        "req_id": "r-42"
    }))
    .expect("parse command");

    match parsed {
        IpcCommand::Send { req_id, .. } => {
            assert_eq!(req_id.as_deref(), Some("r-42"));
        }
        _ => panic!("expected send command"),
    }
}

#[test]
fn parse_peers_command() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"peers"}"#).unwrap();
    assert!(matches!(cmd, IpcCommand::Peers { req_id: None }));
}

#[test]
fn parse_status_command() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"status"}"#).unwrap();
    assert!(matches!(cmd, IpcCommand::Status { req_id: None }));
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
        req_id: None,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["msg_id"], id.to_string());
    assert!(v.get("req_id").is_none());
}

#[test]
fn send_ack_with_req_id() {
    let id = Uuid::new_v4();
    let reply = DaemonReply::SendAck {
        ok: true,
        msg_id: id,
        req_id: Some("r-99".to_string()),
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["req_id"], "r-99");
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
        req_id: None,
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
        req_id: None,
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
        error: IpcErrorCode::PeerNotFound,
        req_id: None,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"], "peer_not_found");
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
        IpcCommand::Hello {
            version, consumer, ..
        } => {
            assert_eq!(version, 2);
            assert_eq!(consumer, "default");
        }
        _ => panic!("expected hello command"),
    }
}

#[test]
fn parse_hello_with_consumer() {
    let cmd: IpcCommand =
        serde_json::from_str(r#"{"cmd":"hello","version":2,"consumer":"my-tool"}"#).unwrap();
    match cmd {
        IpcCommand::Hello { consumer, .. } => {
            assert_eq!(consumer, "my-tool");
        }
        _ => panic!("expected hello command"),
    }
}

#[test]
fn parse_hello_with_req_id() {
    let cmd: IpcCommand =
        serde_json::from_str(r#"{"cmd":"hello","version":2,"req_id":"h-1"}"#).unwrap();
    match cmd {
        IpcCommand::Hello { req_id, .. } => {
            assert_eq!(req_id.as_deref(), Some("h-1"));
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
        IpcCommand::Auth { token, req_id } => {
            assert_eq!(token.len(), 64);
            assert!(req_id.is_none());
        }
        _ => panic!("expected auth command"),
    }
}

#[test]
fn parse_auth_with_req_id() {
    let cmd: IpcCommand = serde_json::from_str(
        r#"{"cmd":"auth","token":"abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234","req_id":"a-1"}"#,
    )
    .unwrap();
    match cmd {
        IpcCommand::Auth { req_id, .. } => {
            assert_eq!(req_id.as_deref(), Some("a-1"));
        }
        _ => panic!("expected auth command"),
    }
}

#[test]
fn parse_whoami_command() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"whoami"}"#).unwrap();
    assert!(matches!(cmd, IpcCommand::Whoami { req_id: None }));
}

#[test]
fn parse_whoami_with_req_id() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"whoami","req_id":"w-1"}"#).unwrap();
    match cmd {
        IpcCommand::Whoami { req_id } => {
            assert_eq!(req_id.as_deref(), Some("w-1"));
        }
        _ => panic!("expected whoami command"),
    }
}

#[test]
fn parse_inbox_command() {
    let cmd: IpcCommand =
        serde_json::from_str(r#"{"cmd":"inbox","limit":100,"kinds":["query","notify"]}"#).unwrap();
    match cmd {
        IpcCommand::Inbox {
            limit,
            kinds,
            req_id,
        } => {
            assert_eq!(limit, 100);
            assert_eq!(kinds.unwrap().len(), 2);
            assert!(req_id.is_none());
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
            kinds,
            req_id,
        } => {
            assert_eq!(limit, 50);
            assert!(kinds.is_none());
            assert!(req_id.is_none());
        }
        _ => panic!("expected inbox command"),
    }
}

#[test]
fn parse_ack_command() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"ack","up_to_seq":42}"#).unwrap();
    match cmd {
        IpcCommand::Ack { up_to_seq, req_id } => {
            assert_eq!(up_to_seq, 42);
            assert!(req_id.is_none());
        }
        _ => panic!("expected ack command"),
    }
}

#[test]
fn parse_ack_with_req_id() {
    let cmd: IpcCommand =
        serde_json::from_str(r#"{"cmd":"ack","up_to_seq":7,"req_id":"k-1"}"#).unwrap();
    match cmd {
        IpcCommand::Ack { up_to_seq, req_id } => {
            assert_eq!(up_to_seq, 7);
            assert_eq!(req_id.as_deref(), Some("k-1"));
        }
        _ => panic!("expected ack command"),
    }
}

#[test]
fn parse_subscribe_command() {
    let cmd: IpcCommand =
        serde_json::from_str(r#"{"cmd":"subscribe","replay":true,"kinds":["query"]}"#).unwrap();
    match cmd {
        IpcCommand::Subscribe {
            replay,
            kinds,
            req_id,
        } => {
            assert!(replay);
            assert_eq!(kinds.unwrap().len(), 1);
            assert!(req_id.is_none());
        }
        _ => panic!("expected subscribe command"),
    }
}

#[test]
fn parse_subscribe_defaults() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"subscribe"}"#).unwrap();
    match cmd {
        IpcCommand::Subscribe {
            replay,
            kinds,
            req_id,
        } => {
            assert!(replay); // default_replay is true
            assert!(kinds.is_none());
            assert!(req_id.is_none());
        }
        _ => panic!("expected subscribe command"),
    }
}

// --- req_id round-trip on IpcCommand accessor ---

#[test]
fn req_id_accessor_returns_none_when_absent() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"peers"}"#).unwrap();
    assert!(cmd.req_id().is_none());
}

#[test]
fn req_id_accessor_returns_value_when_present() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"cmd":"status","req_id":"s-5"}"#).unwrap();
    assert_eq!(cmd.req_id(), Some("s-5"));
}

// --- v2 reply serialization ---

#[test]
fn hello_reply_serialization() {
    let reply = DaemonReply::Hello {
        ok: true,
        version: 2,
        daemon_max_version: 2,
        agent_id: "ed25519.a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".to_string(),
        features: vec!["auth".to_string(), "buffer".to_string()],
        req_id: None,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["version"], 2);
    assert_eq!(v["daemon_max_version"], 2);
    assert_eq!(v["features"].as_array().unwrap().len(), 2);
    assert!(v.get("req_id").is_none());
}

#[test]
fn hello_reply_with_req_id() {
    let reply = DaemonReply::Hello {
        ok: true,
        version: 2,
        daemon_max_version: 2,
        agent_id: "ed25519.aabbccdd".to_string(),
        features: vec![],
        req_id: Some("h-1".to_string()),
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["req_id"], "h-1");
}

#[test]
fn auth_reply_serialization() {
    let reply = DaemonReply::Auth {
        ok: true,
        auth: "accepted".to_string(),
        req_id: None,
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
        info: WhoamiInfo {
            agent_id: "ed25519.a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".to_string(),
            public_key: "base64key".to_string(),
            name: Some("test-agent".to_string()),
            version: "0.1.0".to_string(),
            ipc_version: 2,
            uptime_secs: 3600,
        },
        req_id: None,
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
    let buffered = BufferedMessage {
        seq: 1,
        buffered_at_ms: 1_708_000_000_000,
        envelope,
    };
    let reply = DaemonReply::Inbox {
        ok: true,
        messages: vec![buffered],
        next_seq: Some(2),
        has_more: false,
        req_id: None,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["messages"].as_array().unwrap().len(), 1);
    assert_eq!(v["messages"][0]["seq"], 1);
    assert_eq!(v["messages"][0]["buffered_at_ms"], 1_708_000_000_000u64);
    assert_eq!(v["next_seq"], 2);
    assert_eq!(v["has_more"], false);
}

#[test]
fn ack_reply_serialization() {
    let reply = DaemonReply::Ack {
        ok: true,
        acked_seq: 42,
        req_id: None,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["acked_seq"], 42);
}

#[test]
fn subscribe_reply_serialization() {
    let reply = DaemonReply::Subscribe {
        ok: true,
        subscribed: true,
        replayed: 5,
        replay_to_seq: Some(10),
        req_id: None,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["subscribed"], true);
    assert_eq!(v["replayed"], 5);
    assert_eq!(v["replay_to_seq"], 10);
}

// --- InboundEvent serialization ---

#[test]
fn inbound_event_serialization() {
    let envelope = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Notify,
        json!({"topic":"test"}),
    );
    let reply = DaemonReply::InboundEvent {
        event: "inbound",
        replay: false,
        seq: 7,
        buffered_at_ms: 1_708_000_000_000,
        envelope,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["event"], "inbound");
    assert_eq!(v["replay"], false);
    assert_eq!(v["seq"], 7);
    assert_eq!(v["buffered_at_ms"], 1_708_000_000_000u64);
    assert!(v["envelope"]["kind"].is_string());
    // InboundEvent must NOT have ok or req_id
    assert!(v.get("ok").is_none(), "InboundEvent must not have ok");
    assert!(
        v.get("req_id").is_none(),
        "InboundEvent must not have req_id"
    );
}

#[test]
fn inbound_event_replay_true() {
    let envelope = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Query,
        json!({}),
    );
    let reply = DaemonReply::InboundEvent {
        event: "inbound",
        replay: true,
        seq: 1,
        buffered_at_ms: 100,
        envelope,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["replay"], true);
}

// --- IpcErrorCode serialization ---

#[test]
fn error_code_serializes_as_snake_case() {
    let cases = vec![
        (IpcErrorCode::HelloRequired, "hello_required"),
        (IpcErrorCode::UnsupportedVersion, "unsupported_version"),
        (IpcErrorCode::AuthRequired, "auth_required"),
        (IpcErrorCode::AuthFailed, "auth_failed"),
        (IpcErrorCode::InvalidCommand, "invalid_command"),
        (IpcErrorCode::AckOutOfRange, "ack_out_of_range"),
        (IpcErrorCode::PeerNotFound, "peer_not_found"),
        (IpcErrorCode::PeerUnreachable, "peer_unreachable"),
        (IpcErrorCode::InternalError, "internal_error"),
    ];
    for (code, expected) in cases {
        let json = serde_json::to_value(&code).unwrap();
        assert_eq!(
            json, expected,
            "IpcErrorCode::{code:?} should serialize to {expected}"
        );
    }
}

#[test]
fn error_code_roundtrips() {
    let code = IpcErrorCode::AuthFailed;
    let json = serde_json::to_string(&code).unwrap();
    let parsed: IpcErrorCode = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, code);
}

#[test]
fn error_reply_with_req_id() {
    let reply = DaemonReply::Error {
        ok: false,
        error: IpcErrorCode::HelloRequired,
        req_id: Some("e-1".to_string()),
    };
    let json = serde_json::to_string(&reply).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"], "hello_required");
    assert_eq!(v["req_id"], "e-1");
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
            IpcCommand::Hello { version: v, consumer, .. } => {
                prop_assert_eq!(v, version);
                prop_assert_eq!(consumer, "default");
            }
            _ => prop_assert!(false, "expected hello command"),
        }
    }

    #[test]
    fn subscribe_kinds_roundtrip(
        kinds in proptest::collection::vec(arb_message_kind(), 0..5)
    ) {
        let kinds_str: Vec<String> = kinds.iter().map(|k| {
            let s = serde_json::to_string(k).unwrap();
            // strip outer quotes to get raw string
            s.trim_matches('"').to_string()
        }).collect();
        let kinds_json: Vec<String> = kinds_str.iter().map(|s| format!("\"{s}\"")).collect();
        let cmd_json = format!(r#"{{"cmd":"subscribe","kinds":[{}]}}"#, kinds_json.join(","));
        let parsed: IpcCommand = serde_json::from_str(&cmd_json).unwrap();
        match parsed {
            IpcCommand::Subscribe { kinds: Some(parsed_kinds), .. } => {
                prop_assert_eq!(parsed_kinds.len(), kinds.len());
                for (parsed, original) in parsed_kinds.iter().zip(kinds_str.iter()) {
                    prop_assert_eq!(parsed, original);
                }
            }
            IpcCommand::Subscribe { kinds: None, .. } if kinds.is_empty() => {}
            _ => prop_assert!(false, "expected subscribe command"),
        }
    }

    #[test]
    fn req_id_roundtrips_on_all_commands(req_id in "[a-z0-9\\-]{1,32}") {
        // Test req_id round-trip on a representative set of v2 commands
        let hello = format!(r#"{{"cmd":"hello","version":2,"req_id":"{req_id}"}}"#);
        let parsed: IpcCommand = serde_json::from_str(&hello).unwrap();
        prop_assert_eq!(parsed.req_id(), Some(req_id.as_str()));

        let peers = format!(r#"{{"cmd":"peers","req_id":"{req_id}"}}"#);
        let parsed: IpcCommand = serde_json::from_str(&peers).unwrap();
        prop_assert_eq!(parsed.req_id(), Some(req_id.as_str()));

        let ack = format!(r#"{{"cmd":"ack","up_to_seq":1,"req_id":"{req_id}"}}"#);
        let parsed: IpcCommand = serde_json::from_str(&ack).unwrap();
        prop_assert_eq!(parsed.req_id(), Some(req_id.as_str()));
    }
}
