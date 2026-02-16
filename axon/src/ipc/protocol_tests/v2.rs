use super::*;
use serde_json::Value;
use serde_json::json;

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
        replay_to_seq: 10,
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
