use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

use super::{ResponseMode, daemon_reply_exit_code, is_unsolicited_event, send_ipc};

#[test]
fn daemon_error_maps_to_exit_two() {
    let code = daemon_reply_exit_code(
        &json!({"ok": false, "error": "peer_not_found"}),
        ResponseMode::Generic,
    );
    assert_eq!(code, ExitCode::from(2));
}

#[test]
fn timeout_error_maps_to_exit_three() {
    let code = daemon_reply_exit_code(
        &json!({"ok": false, "error": "timeout"}),
        ResponseMode::Request,
    );
    assert_eq!(code, ExitCode::from(3));
}

#[test]
fn request_with_embedded_error_envelope_maps_to_exit_two() {
    let code = daemon_reply_exit_code(
        &json!({
            "ok": true,
            "msg_id": "550e8400-e29b-41d4-a716-446655440000",
            "response": {"kind": "error", "payload": {"message": "no handler"}}
        }),
        ResponseMode::Request,
    );
    assert_eq!(code, ExitCode::from(2));
}

#[tokio::test]
async fn send_ipc_rejects_oversized_command() {
    let paths = axon::config::AxonPaths {
        root: PathBuf::from("/tmp/axon-test-nonexistent"),
        socket: PathBuf::from("/tmp/axon-test-nonexistent/axon.sock"),
        config: PathBuf::from("/tmp/axon-test-nonexistent/config.yaml"),
        known_peers: PathBuf::from("/tmp/axon-test-nonexistent/known_peers.json"),
        identity_key: PathBuf::from("/tmp/axon-test-nonexistent/identity.key"),
        identity_pub: PathBuf::from("/tmp/axon-test-nonexistent/identity.pub"),
    };

    let big_payload = "x".repeat(70_000);
    let command = json!({"cmd": "send", "to": "ed25519.00000000000000000000000000000000", "kind": "message", "payload": big_payload});
    let err = send_ipc(&paths, command)
        .await
        .expect_err("should reject oversized command");
    assert!(err.to_string().contains("exceeds"), "error: {err}");
}

#[test]
fn unsolicited_event_detection_uses_event_key() {
    assert!(is_unsolicited_event(&json!({"event": "inbound"})));
    assert!(is_unsolicited_event(
        &json!({"event": "pair_request", "agent_id": "ed25519.abc"})
    ));
    assert!(!is_unsolicited_event(&json!({"ok": true})));
}
