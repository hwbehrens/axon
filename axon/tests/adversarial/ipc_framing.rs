use crate::*;

// =========================================================================
// §8 IPC framing boundary conditions
// =========================================================================

/// >64KiB line must produce invalid_command error and close the connection.
/// This verifies the MAX_IPC_LINE_LENGTH enforcement in server.rs.
#[tokio::test]
async fn ipc_overlong_line_rejects_and_closes() {
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("axon.sock");
        let (server, _cmd_rx) =
            IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
                .await
                .unwrap();

        let stream = UnixStream::connect(&socket_path).await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        // Send a line that exceeds 64 KiB (MAX_IPC_LINE_LENGTH)
        let overlong = format!(
            "{{\"cmd\":\"status\",\"data\":\"{}\"}}\n",
            "x".repeat(70_000)
        );
        let _ = write_half.write_all(overlong.as_bytes()).await;

        // Should get an error reply
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.unwrap();
        if n > 0 {
            assert!(
                line.contains("\"ok\":false"),
                "overlong line should get error reply, got: {line}"
            );
        }

        // Connection should be closed — next read returns EOF
        let mut eof_line = String::new();
        let eof_n = reader.read_line(&mut eof_line).await.unwrap();
        assert_eq!(eof_n, 0, "connection must be closed after overlong line");

        drop(server);
    })
    .await;

    assert!(
        result.is_ok(),
        "ipc_overlong_line_rejects_and_closes timed out"
    );
}

/// Partial data without a newline followed by EOF must not panic or hang.
#[tokio::test]
async fn ipc_partial_data_then_eof() {
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("axon.sock");
        let (server, mut cmd_rx) =
            IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
                .await
                .unwrap();

        // Spawn handler so server doesn't block
        let server_clone = server.clone();
        let handler = tokio::spawn(async move {
            while let Some(evt) = cmd_rx.recv().await {
                let _ = server_clone
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

        // Send partial JSON without newline, then close
        {
            let mut stream = UnixStream::connect(&socket_path).await.unwrap();
            stream.write_all(b"{\"cmd\":\"status\"").await.unwrap();
            // Drop stream (EOF without newline)
        }

        // Brief pause, then verify server is still alive
        tokio::time::sleep(Duration::from_millis(100)).await;

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
            "server must still work after partial-data client"
        );

        drop(server);
        handler.abort();
    })
    .await;

    assert!(result.is_ok(), "ipc_partial_data_then_eof timed out");
}

/// Invalid UTF-8 sequences must close the connection without crashing the server.
#[tokio::test]
async fn ipc_invalid_utf8_closes_cleanly() {
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("axon.sock");
        let (server, mut cmd_rx) =
            IpcServer::bind(socket_path.clone(), 64, IpcServerConfig::default())
                .await
                .unwrap();

        let server_clone = server.clone();
        let handler = tokio::spawn(async move {
            while let Some(evt) = cmd_rx.recv().await {
                let _ = server_clone
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

        // Send invalid UTF-8 bytes followed by newline
        {
            let mut stream = UnixStream::connect(&socket_path).await.unwrap();
            stream.write_all(b"\x80\x81\x82\xff\xfe\n").await.unwrap();
            // Read response — should be error or connection close
            let mut buf = vec![0u8; 4096];
            let _ = tokio::time::timeout(
                Duration::from_millis(500),
                tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
            )
            .await;
        }

        // Verify server survives
        tokio::time::sleep(Duration::from_millis(50)).await;
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
            "server must survive invalid UTF-8 on another connection"
        );

        drop(server);
        handler.abort();
    })
    .await;

    assert!(result.is_ok(), "ipc_invalid_utf8_closes_cleanly timed out");
}
