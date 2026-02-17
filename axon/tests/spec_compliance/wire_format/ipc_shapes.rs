use super::*;

/// whoami response includes identity fields.
#[test]
fn ipc_whoami_command_and_response_shape() {
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
            uptime_secs: 123,
        },
        req_id: None,
    };
    let j: Value = serde_json::to_value(&reply).unwrap();
    assert_eq!(j["ok"], true);
    assert_eq!(j["agent_id"], "ed25519.test");
    assert_eq!(j["uptime_secs"], 123);
}
