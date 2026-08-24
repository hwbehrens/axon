use std::path::PathBuf;
use std::time::Duration;

use axon::config::AxonPaths;
use axon::daemon::{DaemonOptions, run_daemon};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_for_socket(path: &std::path::Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "socket did not appear"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn ipc(path: &std::path::Path, command: Value) -> Value {
    let mut stream = UnixStream::connect(path).await.unwrap();
    stream
        .write_all(format!("{command}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn daemon_starts_serves_ipc_and_shuts_down_cleanly() {
    let root = tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(root.path()));
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let root_path = root.path().to_path_buf();
    let task = tokio::spawn(async move {
        run_daemon(DaemonOptions {
            port: Some(free_port()),
            disable_mdns: true,
            axon_root: Some(root_path),
            cancel: Some(task_cancel),
        })
        .await
    });
    wait_for_socket(&paths.socket).await;
    let reply = ipc(&paths.socket, json!({"cmd": "whoami"})).await;
    assert_eq!(reply["ok"], true);
    assert!(reply["agent_id"].as_str().unwrap().starts_with("ed25519."));

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("shutdown timeout")
        .expect("daemon task")
        .expect("daemon result");
    assert!(!paths.socket.exists());
}

#[tokio::test]
async fn daemon_rejects_legacy_peer_cache_without_importing_it() {
    let root = tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(root.path()));
    paths.ensure_root_exists().unwrap();
    std::fs::write(&paths.legacy_known_peers, b"[]").unwrap();
    let error = run_daemon(DaemonOptions {
        port: Some(free_port()),
        disable_mdns: true,
        axon_root: Some(root.path().to_path_buf()),
        cancel: Some(CancellationToken::new()),
    })
    .await
    .expect_err("legacy state must fail closed");
    assert!(error.to_string().contains("re-enroll"));
    assert!(!paths.peers.exists());
}

#[tokio::test]
async fn transport_bind_failure_cleans_up_ipc_and_lock() {
    let root = tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(root.path()));
    let occupied = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = occupied.local_addr().unwrap().port();

    let error = run_daemon(DaemonOptions {
        port: Some(port),
        disable_mdns: true,
        axon_root: Some(root.path().to_path_buf()),
        cancel: Some(CancellationToken::new()),
    })
    .await
    .expect_err("occupied QUIC port must fail startup");

    assert!(error.to_string().contains("bind"));
    assert!(!paths.socket.exists());
    assert!(!root.path().join("daemon.pid").exists());
}
