//! End-to-end daemon tests that exercise cross-cutting integration seams.
//!
//! These tests require the full daemon stack (IPC ↔ daemon ↔ transport ↔ QUIC)
//! and cover scenarios that unit tests cannot catch: IPC broadcast fanout,
//! initiator-rule timing, pubkey pinning at the QUIC layer, concurrent sends,
//! and shutdown under active traffic.
//!
//! These are longer-running e2e tests (~10s).

use std::path::PathBuf;
use std::time::Duration;

use axon::config::{AxonPaths, Config, StaticPeerConfig};
use axon::daemon::{DaemonOptions, run_daemon};
use axon::identity::Identity;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

mod broadcast;
mod connection;

// =========================================================================
// Helpers
// =========================================================================

pub(crate) fn pick_free_port() -> u16 {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.local_addr().unwrap().port()
}

pub(crate) struct DaemonHandle {
    pub(crate) cancel: CancellationToken,
    pub(crate) paths: AxonPaths,
    pub(crate) handle: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl DaemonHandle {
    pub(crate) async fn shutdown(self) {
        self.cancel.cancel();
        let _ = timeout(Duration::from_secs(10), self.handle).await;
    }
}

pub(crate) fn spawn_daemon_with_config(
    dir: &std::path::Path,
    port: u16,
    config: Config,
    agent_id_override: Option<String>,
) -> DaemonHandle {
    let cancel = CancellationToken::new();
    let paths = AxonPaths::from_root(PathBuf::from(dir));
    paths.ensure_root_exists().unwrap();

    let toml = toml::to_string_pretty(&config).unwrap();
    std::fs::write(&paths.config, toml).unwrap();

    let opts = DaemonOptions {
        port: Some(port),
        disable_mdns: true,
        axon_root: Some(PathBuf::from(dir)),
        agent_id: agent_id_override,
        cancel: Some(cancel.clone()),
    };

    let handle = tokio::spawn(async move { run_daemon(opts).await });
    DaemonHandle {
        cancel,
        paths,
        handle,
    }
}

pub(crate) fn spawn_daemon(
    dir: &std::path::Path,
    port: u16,
    peers: Vec<StaticPeerConfig>,
) -> DaemonHandle {
    spawn_daemon_with_config(
        dir,
        port,
        Config {
            port: Some(port),
            peers,
            ..Default::default()
        },
        None,
    )
}

pub(crate) async fn wait_for_socket(paths: &AxonPaths, timeout_dur: Duration) -> bool {
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

/// Poll the peers IPC command until a specific peer shows as "connected", with a timeout.
pub(crate) async fn wait_for_peer_connected(
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
                    p["id"].as_str() == Some(peer_agent_id)
                        && p["status"].as_str() == Some("connected")
                }) {
                    return true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub(crate) async fn ipc_command(
    socket_path: &std::path::Path,
    command: Value,
) -> anyhow::Result<Value> {
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

pub(crate) struct TwoDaemons {
    #[allow(dead_code)]
    pub(crate) id_a: Identity,
    pub(crate) id_b: Identity,
    pub(crate) daemon_a: DaemonHandle,
    pub(crate) daemon_b: DaemonHandle,
}

pub(crate) async fn setup_connected_pair() -> TwoDaemons {
    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();

    let paths_a = AxonPaths::from_root(PathBuf::from(dir_a.path()));
    paths_a.ensure_root_exists().unwrap();
    let id_a = Identity::load_or_generate(&paths_a).unwrap();

    let paths_b = AxonPaths::from_root(PathBuf::from(dir_b.path()));
    paths_b.ensure_root_exists().unwrap();
    let id_b = Identity::load_or_generate(&paths_b).unwrap();

    let port_a = pick_free_port();
    let port_b = pick_free_port();

    let peers_for_a = vec![StaticPeerConfig {
        agent_id: id_b.agent_id().into(),
        addr: format!("127.0.0.1:{port_b}").parse().unwrap(),
        pubkey: id_b.public_key_base64().to_string(),
    }];
    let peers_for_b = vec![StaticPeerConfig {
        agent_id: id_a.agent_id().into(),
        addr: format!("127.0.0.1:{port_a}").parse().unwrap(),
        pubkey: id_a.public_key_base64().to_string(),
    }];

    let daemon_a = spawn_daemon(dir_a.path(), port_a, peers_for_a);
    let daemon_b = spawn_daemon(dir_b.path(), port_b, peers_for_b);

    assert!(wait_for_socket(&daemon_a.paths, Duration::from_secs(5)).await);
    assert!(wait_for_socket(&daemon_b.paths, Duration::from_secs(5)).await);
    assert!(
        wait_for_peer_connected(
            &daemon_a.paths.socket,
            id_b.agent_id(),
            Duration::from_secs(10)
        )
        .await,
        "daemon A did not connect to B"
    );

    // Leak the tempdirs so they survive for the duration of the test.
    // (DaemonHandle holds paths that point into them.)
    std::mem::forget(dir_a);
    std::mem::forget(dir_b);

    TwoDaemons {
        id_a,
        id_b,
        daemon_a,
        daemon_b,
    }
}
