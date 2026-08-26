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
        IpcErrorCode::SendCapacityExceeded,
        IpcErrorCode::MessageTooLarge,
        IpcErrorCode::InternalError,
    ];
    let mut seen = std::collections::HashSet::new();
    for code in codes {
        let message = code.message();
        assert!(!message.is_empty(), "{code:?} has an empty message");
        assert!(seen.insert(message), "duplicate message for {code:?}");
    }
}

// ---------------------------------------------------------------------------
// Round-seven review pin (DEC-022): the outbound line limit includes the
// trailing newline, and every daemon reply/event must pass ONE encoder that
// enforces it — a network envelope accepted under the wire limit grows once
// wrapped in event JSON.
// ---------------------------------------------------------------------------

fn inbound_event_with_payload(payload: &serde_json::Value) -> DaemonReply {
    let envelope = Envelope::new(
        AgentId::parse("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
        AgentId::parse("ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
        crate::message::MessageKind::Message,
        payload.clone(),
    );
    DaemonReply::InboundEvent {
        event: "inbound",
        from: envelope
            .from
            .as_ref()
            .map(|id| id.to_string())
            .unwrap_or_default(),
        envelope,
    }
}

#[test]
fn encode_reply_line_enforces_the_limit_including_newline() {
    // Baseline with an empty payload; every additional 'a' adds exactly one
    // byte to the serialization (no JSON escaping), so exact boundary
    // lengths are constructible deterministically.
    let base = serde_json::to_string(&inbound_event_with_payload(&serde_json::json!("")))
        .expect("baseline serializes")
        .len();
    let payload_for = |total_body: usize| serde_json::json!("a".repeat(total_body - base));

    // Body of exactly 65,535 bytes + newline = the 65,536-byte frame limit.
    let fits = inbound_event_with_payload(&payload_for(MAX_IPC_LINE_LENGTH - 1));
    assert!(
        encode_reply_line(&fits).is_ok(),
        "a body of MAX-1 bytes plus newline is exactly at the limit"
    );

    // One byte more must be refused — never truncated.
    let over = inbound_event_with_payload(&payload_for(MAX_IPC_LINE_LENGTH));
    match encode_reply_line(&over) {
        Err(EncodeLineError::TooLarge(_)) => {}
        Err(EncodeLineError::Serialize(err)) => panic!("unexpected serialize error: {err}"),
        Ok(line) => panic!("oversized line must be refused, got {} bytes", line.len()),
    }
}

#[test]
fn daemon_reply_req_id_extracts_correlation_ids() {
    assert_eq!(
        DaemonReply::SendOk {
            ok: true,
            msg_id: Uuid::new_v4(),
            req_id: Some("req-1".to_string()),
            response: None,
        }
        .req_id(),
        Some("req-1")
    );
    assert_eq!(
        inbound_event_with_payload(&serde_json::json!({})).req_id(),
        None
    );
}

#[test]
fn error_reply_line_drops_an_overbound_echo_instead_of_panicking() {
    // A pathological req_id (bounded at ingress; defended here) must never
    // make a terminal error reply unframeable: the echo is dropped, the
    // static error body always fits. This pins the round-eight P1 where the
    // message_too_large fallback could itself exceed the limit and panic.
    let line = error_reply_line(IpcErrorCode::MessageTooLarge, Some("r".repeat(70_000)))
        .expect("must encode");
    assert!(line.len() < MAX_IPC_LINE_LENGTH);
    let decoded: serde_json::Value = serde_json::from_str(&line).expect("json");
    assert_eq!(decoded["error"], "message_too_large");
    assert!(
        decoded.get("req_id").is_none(),
        "the overbound echo must be dropped, never truncated"
    );

    // An in-bound req_id is preserved verbatim.
    let echoed = "r".repeat(MAX_REQ_ID_BYTES);
    let line =
        error_reply_line(IpcErrorCode::MessageTooLarge, Some(echoed.clone())).expect("must encode");
    let decoded: serde_json::Value = serde_json::from_str(&line).expect("json");
    assert_eq!(decoded["req_id"], serde_json::json!(echoed));
}
