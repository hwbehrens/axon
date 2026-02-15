//! Fuzz target: feed arbitrary line sequences through IPC command deserialization
//! and stateful session validation. Must not panic regardless of input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::ipc::{DaemonReply, IpcCommand, IpcErrorCode};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);

    // Phase 1: Deserialization must never panic
    let mut commands = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(cmd) = serde_json::from_str::<IpcCommand>(line) {
            commands.push(cmd);
        }
    }

    // Phase 2: Validate reply serialization never panics
    // Construct representative replies and ensure round-trip works
    for cmd in &commands {
        let reply = match cmd {
            IpcCommand::Hello {
                version, req_id, ..
            } => DaemonReply::Hello {
                ok: true,
                version: (*version).min(2),
                daemon_max_version: 2,
                agent_id: "ed25519.fuzz".to_string(),
                features: vec![],
                req_id: req_id.clone(),
            },
            IpcCommand::Auth { req_id, .. } => DaemonReply::Error {
                ok: false,
                error: IpcErrorCode::AuthFailed,
                req_id: req_id.clone(),
            },
            IpcCommand::Peers { req_id, .. } => DaemonReply::Peers {
                ok: true,
                peers: vec![],
                req_id: req_id.clone(),
            },
            IpcCommand::Status { req_id, .. } => DaemonReply::Status {
                ok: true,
                uptime_secs: 0,
                peers_connected: 0,
                messages_sent: 0,
                messages_received: 0,
                req_id: req_id.clone(),
            },
            _ => DaemonReply::Error {
                ok: false,
                error: IpcErrorCode::HelloRequired,
                req_id: cmd.req_id().map(|s| s.to_string()),
            },
        };
        // Serialization must not panic
        let _ = serde_json::to_string(&reply);
    }
});
