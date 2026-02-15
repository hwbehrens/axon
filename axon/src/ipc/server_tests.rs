use super::*;
use crate::message::MessageKind;
use serde_json::json;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::test]
async fn bind_creates_socket_file() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let (_server, _rx) = IpcServer::bind(socket_path.clone(), 64)
        .await
        .expect("bind IPC server");

    assert!(socket_path.exists());
}

#[tokio::test]
async fn broadcasts_inbound_to_multiple_clients() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64)
        .await
        .expect("bind IPC server");

    let mut client_a = UnixStream::connect(&socket_path)
        .await
        .expect("connect client A");
    let mut client_b = UnixStream::connect(&socket_path)
        .await
        .expect("connect client B");

    // Give the accept loop a moment to register clients.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let envelope = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Notify,
        json!({"topic":"meta.status", "data":{}}),
    );
    server
        .broadcast_inbound(&envelope)
        .await
        .expect("broadcast inbound");

    let mut line_a = String::new();
    let mut line_b = String::new();
    let mut reader_a = BufReader::new(&mut client_a);
    let mut reader_b = BufReader::new(&mut client_b);
    reader_a.read_line(&mut line_a).await.expect("read A");
    reader_b.read_line(&mut line_b).await.expect("read B");

    assert!(line_a.contains("\"inbound\":true"));
    assert!(line_b.contains("\"inbound\":true"));
}

#[tokio::test]
async fn send_command_round_trip() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let (server, mut cmd_rx) = IpcServer::bind(socket_path.clone(), 64)
        .await
        .expect("bind IPC server");

    let mut client = UnixStream::connect(&socket_path).await.expect("connect");

    client
        .write_all(b"{\"cmd\":\"peers\"}\n")
        .await
        .expect("write");

    let cmd = tokio::time::timeout(std::time::Duration::from_secs(2), cmd_rx.recv())
        .await
        .expect("timeout")
        .expect("recv");

    assert!(matches!(cmd.command, IpcCommand::Peers));

    server
        .send_reply(
            cmd.client_id,
            &DaemonReply::Peers {
                ok: true,
                peers: vec![],
            },
        )
        .await
        .expect("reply");

    let mut line = String::new();
    let mut reader = BufReader::new(&mut client);
    reader.read_line(&mut line).await.expect("read");
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["peers"], json!([]));
}

#[tokio::test]
async fn invalid_command_returns_error() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let (_server, _rx) = IpcServer::bind(socket_path.clone(), 64)
        .await
        .expect("bind IPC server");

    let mut client = UnixStream::connect(&socket_path).await.expect("connect");
    client
        .write_all(b"{\"cmd\":\"unknown\"}\n")
        .await
        .expect("write");

    let mut line = String::new();
    let mut reader = BufReader::new(client);
    reader.read_line(&mut line).await.expect("read");
    assert!(line.contains("\"ok\":false"));
    assert!(line.contains("invalid command"));
}

#[tokio::test]
async fn cleanup_removes_socket() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64)
        .await
        .expect("bind IPC server");

    assert!(socket_path.exists());
    server.cleanup_socket().expect("cleanup");
    assert!(!socket_path.exists());
}

#[tokio::test]
async fn client_disconnect_does_not_affect_others() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon.sock");
    let (server, _rx) = IpcServer::bind(socket_path.clone(), 64)
        .await
        .expect("bind IPC server");

    let client_a = UnixStream::connect(&socket_path).await.expect("connect A");
    let mut client_b = UnixStream::connect(&socket_path).await.expect("connect B");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(server.client_count().await, 2);

    drop(client_a);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let envelope = Envelope::new(
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        MessageKind::Ping,
        json!({}),
    );
    server
        .broadcast_inbound(&envelope)
        .await
        .expect("broadcast");

    let mut line = String::new();
    let mut reader = BufReader::new(&mut client_b);
    reader.read_line(&mut line).await.expect("read B");
    assert!(line.contains("\"inbound\":true"));
}
