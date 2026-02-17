//! Daemon lifecycle integration tests.
//!
//! These tests exercise full daemon startup, shutdown, and reconnection
//! behaviors. They spin up real daemon instances in spawned tasks with
//! temp directories and CancellationTokens for controlled lifecycle.
//!
//! These are longer-running e2e tests (~10s).

use std::path::{Path, PathBuf};
use std::time::Duration;

use axon::config::{AxonPaths, StaticPeerConfig};
use axon::daemon::{DaemonOptions, run_daemon};
use axon::identity::Identity;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

#[path = "daemon_lifecycle/basic.rs"]
mod basic;
#[path = "daemon_lifecycle/messaging.rs"]
mod messaging;

// =========================================================================
// Helpers
// =========================================================================

/// Bind a UDP socket to port 0 and return the OS-assigned port.
fn pick_free_port() -> u16 {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.local_addr().unwrap().port()
}

fn axon_bin() -> PathBuf {
    if let Some(bin) = std::env::var_os("CARGO_BIN_EXE_axon") {
        return PathBuf::from(bin);
    }

    let current = std::env::current_exe().expect("resolve current test executable");
    let debug_dir = current
        .parent()
        .and_then(Path::parent)
        .expect("resolve target debug dir");
    let fallback = if cfg!(windows) {
        debug_dir.join("axon.exe")
    } else {
        debug_dir.join("axon")
    };
    assert!(
        fallback.exists(),
        "failed to locate axon binary via CARGO_BIN_EXE_axon and fallback path {}",
        fallback.display()
    );
    fallback
}

/// Start a daemon in a background task, returning its cancel token and paths.
fn spawn_daemon(
    dir: &std::path::Path,
    port: u16,
    disable_mdns: bool,
    peers: Vec<StaticPeerConfig>,
) -> (
    CancellationToken,
    AxonPaths,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let cancel = CancellationToken::new();
    let paths = AxonPaths::from_root(PathBuf::from(dir));
    paths.ensure_root_exists().unwrap();

    // Write static peer config if any.
    if !peers.is_empty() {
        let config = axon::config::Config {
            port: Some(port),
            peers,
            ..Default::default()
        };
        let toml = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&paths.config, toml).unwrap();
    }

    let opts = DaemonOptions {
        port: Some(port),
        disable_mdns,
        axon_root: Some(PathBuf::from(dir)),
        cancel: Some(cancel.clone()),
    };

    let handle = tokio::spawn(async move { run_daemon(opts).await });

    (cancel, paths, handle)
}

/// Wait until the IPC socket file appears, with a timeout.
async fn wait_for_socket(paths: &AxonPaths, timeout_dur: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout_dur;
    loop {
        if paths.socket.exists() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Poll the peers IPC command until a specific peer is NOT "connected", with a timeout.
async fn wait_for_peer_disconnected(
    socket_path: &std::path::Path,
    peer_agent_id: &str,
    timeout_dur: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout_dur;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        if let Ok(resp) = ipc_command(socket_path, json!({"cmd": "peers"})).await {
            if let Some(peers) = resp["peers"].as_array() {
                let is_connected = peers.iter().any(|p| {
                    p["agent_id"].as_str() == Some(peer_agent_id)
                        && p["status"].as_str() == Some("connected")
                });
                if !is_connected {
                    return true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Poll the peers IPC command until a specific peer shows as "connected", with a timeout.
async fn wait_for_peer_connected(
    socket_path: &std::path::Path,
    peer_agent_id: &str,
    timeout_dur: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout_dur;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        if let Ok(resp) = ipc_command(socket_path, json!({"cmd": "peers"})).await {
            if let Some(peers) = resp["peers"].as_array() {
                if peers.iter().any(|p| {
                    p["agent_id"].as_str() == Some(peer_agent_id)
                        && p["status"].as_str() == Some("connected")
                }) {
                    return true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Send a JSON command over IPC and read one response line.
async fn ipc_command(socket_path: &std::path::Path, command: Value) -> anyhow::Result<Value> {
    let mut stream = UnixStream::connect(socket_path).await?;
    let line = serde_json::to_string(&command)?;
    stream.write_all(line.as_bytes()).await?;
    stream.write_all(b"\n").await?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    let bytes = timeout(Duration::from_secs(5), reader.read_line(&mut response)).await??;
    if bytes == 0 {
        anyhow::bail!("daemon closed connection");
    }
    Ok(serde_json::from_str(response.trim())?)
}
