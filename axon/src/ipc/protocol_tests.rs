use serde_json::json;

use super::*;

fn sample_command(name: &str) -> IpcCommand {
    let to = AgentId::parse("ed25519.0123456789abcdef0123456789abcdef").unwrap();
    match name {
        "send" => IpcCommand::Send {
            to,
            kind: IpcSendKind::Message,
            payload: json!({}),
            timeout_secs: None,
            ref_id: None,
            req_id: Some("r1".into()),
        },
        "peers" => IpcCommand::Peers { req_id: None },
        "status" => IpcCommand::Status { req_id: None },
        "whoami" => IpcCommand::Whoami { req_id: None },
        "add_peer" => IpcCommand::AddPeer {
            agent_id: Some(to),
            token: None,
            req_id: None,
        },
        "remove_peer" => IpcCommand::RemovePeer {
            agent_id: to,
            req_id: None,
        },
        "serve" => IpcCommand::Serve { req_id: None },
        _ => IpcCommand::Reply {
            request_id: Uuid::new_v4(),
            peer: None,
            kind: IpcReplyKind::Response,
            payload: json!({}),
            req_id: None,
        },
    }
}

#[test]
fn every_command_reports_its_wire_name() {
    for (name, command) in [
        ("send", sample_command("send")),
        ("peers", sample_command("peers")),
        ("status", sample_command("status")),
        ("whoami", sample_command("whoami")),
        ("add_peer", sample_command("add_peer")),
        ("remove_peer", sample_command("remove_peer")),
        ("serve", sample_command("serve")),
        ("reply", sample_command("reply")),
    ] {
        assert_eq!(command.cmd_name(), name, "wire name drift for {name}");
    }
}

#[test]
fn req_id_survives_the_wire_and_attaches_to_the_command() {
    let to = AgentId::parse("ed25519.0123456789abcdef0123456789abcdef").unwrap();
    let wire = serde_json::to_string(&json!({
        "cmd": "send",
        "to": to.as_str(),
        "kind": "message",
        "payload": {},
        "req_id": "r1"
    }))
    .expect("serialize wire form");

    let parsed: IpcCommand = serde_json::from_str(&wire).expect("parse command");
    assert_eq!(parsed.req_id(), Some("r1"));
    assert_eq!(parsed.cmd_name(), "send");
}

#[test]
fn commands_without_req_id_report_none() {
    assert_eq!(sample_command("peers").req_id(), None);
    assert_eq!(sample_command("serve").req_id(), None);
}

#[test]
fn reply_kind_maps_to_message_kinds() {
    assert_eq!(IpcSendKind::Request.as_message_kind(), MessageKind::Request);
    assert_eq!(IpcSendKind::Message.as_message_kind(), MessageKind::Message);
    assert_eq!(
        IpcReplyKind::Response.as_message_kind(),
        MessageKind::Response
    );
    assert_eq!(IpcReplyKind::Error.as_message_kind(), MessageKind::Error);
}

#[test]
fn error_code_messages_are_distinct_and_nonempty() {
    let codes = [
        IpcErrorCode::InvalidCommand,
        IpcErrorCode::CommandTooLarge,
        IpcErrorCode::PeerNotFound,
        IpcErrorCode::PeerNotObserved,
        IpcErrorCode::PeerConflict,
        IpcErrorCode::SelfSend,
        IpcErrorCode::PeerUnreachable,
        IpcErrorCode::Timeout,
        IpcErrorCode::HandlerBusy,
        IpcErrorCode::NotHandler,
        IpcErrorCode::RequestNotFound,
        IpcErrorCode::InternalError,
    ];
    let mut seen = std::collections::HashSet::new();
    for code in codes {
        let message = code.message();
        assert!(!message.is_empty(), "{code:?} has an empty message");
        assert!(seen.insert(message), "duplicate message for {code:?}");
    }
}
