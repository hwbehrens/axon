//! Overlong IPC line handling under hostile client timing.
//!
//! Pins the P1 contract: a client that sends an overlong line without a
//! terminating newline and then pauses must receive the queued
//! `command_too_large` error and be disconnected within the bounded drain
//! window — it may not hold one of the 64 IPC client slots indefinitely,
//! and daemon shutdown must not be able to block on it.

use super::*;

#[tokio::test]
async fn paused_overlong_line_client_is_replied_then_closed_within_bound() {
    let (daemon_a, _daemon_b) = prepare_pair().await;
    let socket = daemon_a.paths.socket.clone();

    let stream = UnixStream::connect(&socket).await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);

    // Exactly the line limit, with no newline: the server detects the
    // overlong line and drains before closing. The client then goes silent,
    // so the drain can only end via its deadline.
    write
        .write_all(&vec![b'a'; MAX_IPC_LINE_LENGTH])
        .await
        .unwrap();

    let started = std::time::Instant::now();
    let mut line = String::new();
    let reply = tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .expect("server must reply within the bound instead of hanging on the drain")
        .expect("reply read must not fail");
    assert!(
        reply > 0,
        "expected the command_too_large reply line before EOF"
    );
    assert!(
        line.contains("command_too_large"),
        "expected command_too_large reply, got: {line}"
    );

    // EOF (client slot released) must follow promptly after the reply.
    let mut eof_buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(5), reader.read(&mut eof_buf))
        .await
        .expect("EOF must arrive after the drain deadline");
    assert_eq!(
        n.expect("read after reply"),
        0,
        "expected connection close after drained reply"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "close took {:?}; drain deadline is not being enforced",
        started.elapsed()
    );

    // The daemon must still accept fresh clients: the slot was released.
    let ack = ipc_command(&socket, json!({"cmd": "status"})).await;
    assert_eq!(ack["ok"], json!(true));

    daemon_a.stop().await;
    _daemon_b.stop().await;
}
