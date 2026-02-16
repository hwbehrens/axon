//! Fuzz target: drive IPC command sequences through the real IpcHandlers
//! state machine. Exercises hello/auth/req_id gating, subscribe, inbox,
//! ack, and broadcast interactions. Must not panic regardless of input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::ipc::{CommandEvent, IpcCommand, IpcServer, IpcServerConfig};
use axon::message::{Envelope, MessageKind};

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() || lines.len() > 50 {
        return;
    }

    // Parse commands first (no runtime needed)
    let mut commands = Vec::new();
    for line in &lines {
        if let Ok(cmd) = serde_json::from_str::<IpcCommand>(line.trim()) {
            commands.push(cmd);
        }
    }
    if commands.is_empty() {
        return;
    }

    // Build a tokio runtime for async operations
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let dir = std::env::temp_dir().join(format!("axon-fuzz-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let socket_path = dir.join("fuzz.sock");
        let _ = std::fs::remove_file(&socket_path);

        let config = IpcServerConfig {
            allow_v1: true,
            buffer_size: 100,
            buffer_ttl_secs: 60,
            ..IpcServerConfig::default().with_token(Some(TOKEN.to_string()))
        };

        let Ok((server, _rx)) = IpcServer::bind(socket_path.clone(), 8, config).await else {
            return;
        };

        let client_id = 1u64;

        // Optionally broadcast a message into the buffer for inbox/subscribe to find
        let envelope = Envelope::new(
            "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            MessageKind::Message,
            serde_json::json!({"topic": "fuzz", "data": {}, "importance": "low"}),
        );
        let _ = server.broadcast_inbound(&envelope).await;

        // Drive each command through the real state machine
        for command in commands {
            let event = CommandEvent {
                client_id,
                command,
            };
            // handle_command applies full policy: hello gating, auth gating,
            // req_id enforcement, subscribe replay, inbox/ack semantics
            let _ = server.handle_command(event).await;
        }

        // Cleanup
        let _ = server.cleanup_socket();
        let _ = std::fs::remove_dir_all(&dir);
    });
});
