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
