use crate::*;

// =========================================================================
// §1 Concurrent IPC flood
// =========================================================================

/// 50 concurrent clients each send 10 rapid status commands.
/// Server must handle all without panics, deadlocks, or lost replies.
#[tokio::test]
async fn concurrent_ipc_flood() {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("axon.sock");
        let (server, mut cmd_rx) =
            IpcServer::bind(socket_path.clone(), 128, IpcServerConfig::default())
                .await
                .unwrap();

        let num_clients = 50;
        let cmds_per_client = 10;

        // Spawn command handler — replies with Status for every command.
        let server_for_handler = server.clone();
        let handler = tokio::spawn(async move {
            let mut count = 0u64;
            while let Some(evt) = cmd_rx.recv().await {
                count += 1;
                let _ = server_for_handler
                    .send_reply(
                        evt.client_id,
                        &DaemonReply::Status {
                            ok: true,
                            uptime_secs: count,
                            peers_connected: 0,
                            messages_sent: 0,
                            messages_received: 0,
                            req_id: None,
                        },
                    )
                    .await;
            }
            count
        });

        // Spawn 50 clients, each sending 10 commands.
        // Each client interleaves send/read to avoid overflowing the
        // internal command channel (bounded at 256).
        let mut client_handles = Vec::new();
        for _ in 0..num_clients {
            let path = socket_path.clone();
            client_handles.push(tokio::spawn(async move {
                let stream = UnixStream::connect(&path).await.unwrap();
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);

                let mut replies = 0;
                let mut line = String::new();
                for _ in 0..cmds_per_client {
                    write_half
                        .write_all(b"{\"cmd\":\"status\"}\n")
                        .await
                        .unwrap();
                    line.clear();
                    reader.read_line(&mut line).await.unwrap();
                    let v: Value = serde_json::from_str(line.trim()).unwrap();
                    assert_eq!(v["ok"], true, "reply should have ok=true");
                    replies += 1;
                }
                replies
            }));
        }

        let mut total_replies = 0;
        for handle in client_handles {
            let count = handle.await.unwrap();
            total_replies += count;
        }

        assert_eq!(
            total_replies,
            num_clients * cmds_per_client,
            "every client must receive all its replies"
        );

        // The accept loop and per-client tasks are spawned (not owned),
        // so the command channel won't close just from dropping the server.
        // Abort the handler to avoid blocking.
        handler.abort();
        drop(server);
    })
    .await;

    assert!(result.is_ok(), "concurrent_ipc_flood timed out (30s)");
}

// =========================================================================
// §3 IPC malformed input resilience
// =========================================================================

/// Server survives malformed IPC inputs and continues serving valid commands.
/// Note: non-UTF8 bytes cause tokio's `lines()` to return an I/O error,
/// which closes that connection. We test those on separate connections.
#[tokio::test]
async fn ipc_malformed_input_resilience() {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("axon.sock");
        let (server, mut cmd_rx) =
            IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
                .await
                .unwrap();

        // Spawn handler for valid commands.
        let server_clone = server.clone();
        let handler = tokio::spawn(async move {
            while let Some(evt) = cmd_rx.recv().await {
                let _ = server_clone
                    .send_reply(
                        evt.client_id,
                        &DaemonReply::Status {
                            ok: true,
                            uptime_secs: 42,
                            peers_connected: 0,
                            messages_sent: 0,
                            messages_received: 0,
                            req_id: None,
                        },
                    )
                    .await;
            }
        });

        // --- UTF-8–safe malformed inputs on a single connection ---
        {
            let stream = UnixStream::connect(&socket_path).await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            let malformed_inputs: Vec<&[u8]> = vec![
                // Empty string
                b"\n",
                // Valid JSON but not a valid command
                b"{\"cmd\":\"nonexistent\"}\n",
                // Partial JSON
                b"{\"cmd\":\"sta\n",
                // Null bytes (valid UTF-8)
                b"\0\0\0\n",
            ];

            for input in &malformed_inputs {
                write_half.write_all(input).await.unwrap();
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                assert!(
                    line.contains("\"ok\":false"),
                    "malformed input should get error reply, got: {line}"
                );
            }
        }

        // --- Overlong line on a separate connection (closes connection) ---
        // Bounded read: overlong lines close the connection since there's no
        // reliable way to find the next command boundary in the stream.
        {
            let stream = UnixStream::connect(&socket_path).await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            let long_line = format!("{}\n", "a".repeat(100_000));
            // Server may close before we finish writing — ignore BrokenPipe
            let _ = write_half.write_all(long_line.as_bytes()).await;

            // Should get an error reply then the connection closes (EOF)
            let mut line = String::new();
            let n = reader.read_line(&mut line).await.unwrap();
            if n > 0 {
                assert!(
                    line.contains("\"ok\":false"),
                    "long malformed input should get error reply, got: {line}"
                );
            }
            // Connection is closed — subsequent reads return EOF or ConnectionReset
            let mut eof_line = String::new();
            let eof_result = reader.read_line(&mut eof_line).await;
            match eof_result {
                Ok(0) => {}                                                     // clean EOF
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {} // RST on Linux
                Ok(n) => panic!("expected EOF after overlong line, got {n} bytes: {eof_line}"),
                Err(e) => panic!("unexpected error after overlong line: {e}"),
            }
        }

        // --- Non-UTF8 binary garbage on a separate connection ---
        // This closes the connection (tokio lines() rejects invalid UTF-8),
        // but must not crash the server.
        {
            let mut stream = UnixStream::connect(&socket_path).await.unwrap();
            stream.write_all(b"\x80\x81\x82\xff\xfe\n").await.unwrap();
            // Connection will be closed by the server; just wait for EOF.
            let mut buf = vec![0u8; 1024];
            let _ = tokio::time::timeout(
                Duration::from_millis(500),
                tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
            )
            .await;
        }

        // --- Verify server is still alive after binary garbage killed a connection ---
        {
            let stream = UnixStream::connect(&socket_path).await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);

            write_half
                .write_all(b"{\"cmd\":\"status\"}\n")
                .await
                .unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let v: Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(
                v["ok"], true,
                "server must still work after binary garbage on another connection"
            );
        }

        drop(server);
        handler.abort();
    })
    .await;

    assert!(
        result.is_ok(),
        "ipc_malformed_input_resilience timed out (10s)"
    );
}

// =========================================================================
// §7 IPC client disconnect under load
// =========================================================================

/// Some clients disconnect abruptly while others keep working.
/// Surviving clients and broadcast_inbound must continue functioning.
#[tokio::test]
async fn ipc_client_disconnect_under_load() {
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("axon.sock");
        let (server, mut cmd_rx) =
            IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
                .await
                .unwrap();

        // Spawn command handler.
        let server_for_handler = server.clone();
        let handler = tokio::spawn(async move {
            while let Some(evt) = cmd_rx.recv().await {
                let _ = server_for_handler
                    .send_reply(
                        evt.client_id,
                        &DaemonReply::Status {
                            ok: true,
                            uptime_secs: 1,
                            peers_connected: 0,
                            messages_sent: 0,
                            messages_received: 0,
                            req_id: None,
                        },
                    )
                    .await;
            }
        });

        // Connect 10 clients — first 5 will disconnect, last 5 will survive.
        let mut disconnect_streams = Vec::new();
        let mut surviving_streams = Vec::new();

        for _ in 0..5 {
            disconnect_streams.push(UnixStream::connect(&socket_path).await.unwrap());
        }
        for _ in 0..5 {
            surviving_streams.push(UnixStream::connect(&socket_path).await.unwrap());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            server.client_count().await,
            10,
            "all 10 clients should be connected"
        );

        // Abruptly disconnect 5 clients while surviving ones send commands.
        let send_task = {
            let socket_path = socket_path.clone();
            tokio::spawn(async move {
                // Give disconnect a moment.
                tokio::time::sleep(Duration::from_millis(10)).await;

                // Use the surviving streams (we pass them via a channel trick).
                // Actually, we need to send on the surviving streams. Let's
                // do it inline after dropping.
                socket_path // return for type
            })
        };

        // Drop the 5 disconnect clients abruptly.
        drop(disconnect_streams);
        send_task.await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Surviving clients should still work.
        for stream in &mut surviving_streams {
            stream.write_all(b"{\"cmd\":\"status\"}\n").await.unwrap();
        }

        for stream in &mut surviving_streams {
            let mut line = String::new();
            let mut reader = BufReader::new(stream);
            reader.read_line(&mut line).await.unwrap();
            let v: Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(v["ok"], true, "surviving client must receive reply");
        }

        // Broadcast should reach surviving clients.
        let envelope = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Message,
            json!({"topic": "meta.status", "data": {}}),
        );
        server.broadcast_inbound(&envelope).await.unwrap();

        for stream in &mut surviving_streams {
            let mut line = String::new();
            let mut reader = BufReader::new(stream);
            reader.read_line(&mut line).await.unwrap();
            assert!(
                line.contains("\"inbound\":true"),
                "surviving client must receive broadcast"
            );
        }

        drop(server);
        handler.abort();
    })
    .await;

    assert!(
        result.is_ok(),
        "ipc_client_disconnect_under_load timed out (15s)"
    );
}
