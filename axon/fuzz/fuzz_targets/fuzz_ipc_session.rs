//! Fuzz target: parse newline-delimited IPC commands and exercise the IPC
//! server's reply-encoding, whoami-composition, and broadcast surfaces.
//! Must not panic regardless of input.

#![no_main]

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;

use axon::ipc::{IpcCommand, IpcErrorCode, IpcServer, IpcServerConfig, error_reply_line};
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
            version: "0.5.0".to_string(),
            max_client_queue: 64,
            uptime_secs: Arc::new(|| 0),
        };

        let Ok((server, _rx)) = IpcServer::bind(socket_path.clone(), 8, config).await else {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };

        // Typed Agent IDs: parse failures are impossible for these fixed
        // literals, and an unreachable! keeps the target panic-free by
        // construction only if parsing stays infallible here.
        let from = axon::message::AgentId::parse("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .expect("fixed literal parses");
        let to = axon::message::AgentId::parse("ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .expect("fixed literal parses");
        let envelope = Envelope::new(
            from,
            to,
            MessageKind::Message,
            serde_json::json!({"topic": "fuzz", "data": {}}),
        );
        let _ = server.broadcast_inbound(&envelope).await;

        for command in commands {
            // The daemon's command handler composes replies; the
            // panic-prone surface is the shared outbound encoder with
            // fuzzer-controlled req_ids (the DEC-023 oversized-echo
            // fallback) plus whoami composition. The no-echo fallback must
            // keep every error reply encodable without panicking.
            let _ = error_reply_line(
                IpcErrorCode::InternalError,
                command.req_id().map(str::to_string),
            );
        }
        let _ = server.whoami_info();

        let _ = server.cleanup_socket();
        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_dir_all(&dir);
    });
});
