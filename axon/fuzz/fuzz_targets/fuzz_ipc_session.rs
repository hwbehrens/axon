//! Fuzz target: parse newline-delimited IPC commands and exercise IPC server
//! command handling/broadcast surfaces. Must not panic regardless of input.

#![no_main]

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;

use axon::ipc::{CommandEvent, IpcCommand, IpcServer, IpcServerConfig};
use axon::message::{Envelope, MessageKind};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    if lines.is_empty() || lines.len() > 50 {
        return;
    }

    let mut commands = Vec::new();
    for line in &lines {
        if let Ok(cmd) = serde_json::from_str::<IpcCommand>(line.trim()) {
            commands.push(cmd);
        }
    }
    if commands.is_empty() {
        return;
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    let case_id = hasher.finish();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let dir = std::env::temp_dir().join(format!("axon-fuzz-{}-{case_id}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let socket_path = dir.join("fuzz.sock");
        let _ = std::fs::remove_file(&socket_path);

        let config = IpcServerConfig {
            agent_id: "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            public_key: "cHVia2V5".to_string(),
            name: Some("fuzz".to_string()),
            version: "0.3.0".to_string(),
            max_client_queue: 64,
            uptime_secs: Arc::new(|| 0),
        };

        let Ok((server, _rx)) = IpcServer::bind(socket_path.clone(), 8, config).await else {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };

        let envelope = Envelope::new(
            "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            MessageKind::Message,
            serde_json::json!({"topic": "fuzz", "data": {}}),
        );
        let _ = server.broadcast_inbound(&envelope).await;

        for command in commands {
            let _ = server
                .handle_command(CommandEvent {
                    client_id: 1,
                    command,
                })
                .await;
        }

        let _ = server.cleanup_socket();
        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_dir_all(&dir);
    });
});
