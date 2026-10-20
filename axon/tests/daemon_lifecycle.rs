use std::path::{Path, PathBuf};
use std::time::Duration;

use axon::config::AxonPaths;
use axon::daemon::{DaemonOptions, run_daemon};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

/// Startup attempts before giving up on a probed port that another host
/// process took between the probe and the daemon's UDP bind. Each attempt
/// probes a fresh port, so a load spike cannot fail the gate with
/// "failed to bind QUIC endpoint", while a persistent conflict still fails
/// loudly on the last attempt.
const PORT_BIND_ATTEMPTS: usize = 3;

fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// True when a failed daemon start is an address-in-use bind conflict: the
/// probe-to-bind race that retrying with a fresh port can win.
fn is_port_conflict(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse)
    })
}

/// Start a daemon for `root` and wait until it serves IPC, retrying with a
/// fresh port when the probed port was taken before the daemon bound it.
async fn spawn_daemon(
    root: &Path,
    paths: &AxonPaths,
    cancel: &CancellationToken,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    spawn_daemon_with(root, paths, cancel, free_port).await
}

async fn spawn_daemon_with(
    root: &Path,
    paths: &AxonPaths,
    cancel: &CancellationToken,
    mut next_port: impl FnMut() -> u16,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    let mut attempt = 1;
    loop {
        let port = next_port();
        let task_cancel = cancel.clone();
        let root_path = root.to_path_buf();
        let mut task = Some(tokio::spawn(async move {
            run_daemon(DaemonOptions {
                port: Some(port),
                disable_mdns: true,
                axon_root: Some(root_path),
                cancel: Some(task_cancel),
                max_inflight_sends: None,
            })
            .await
        }));
        match wait_for_ready(&paths.socket, &mut task).await {
            Ok(()) => return task.take().expect("ready task present"),
            Err(error) if attempt < PORT_BIND_ATTEMPTS && is_port_conflict(&error) => {
                eprintln!(
                    "[diag] daemon startup hit a port conflict (attempt \
                     {attempt}/{PORT_BIND_ATTEMPTS}): {error:#}; retrying with a fresh port"
                );
                attempt += 1;
            }
            Err(error) => panic!("daemon failed to start after {attempt} attempt(s): {error:#}"),
        }
    }
}

/// Wait until the daemon serves IPC. The `whoami` round trip is handled by
/// the daemon's command loop, which starts only after the QUIC endpoint is
/// bound; on early exit the daemon's own startup error is returned so
/// callers can classify it.
async fn wait_for_ready(
    socket: &Path,
    task: &mut Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if task.as_ref().is_some_and(|task| task.is_finished()) {
            let task = task.take().expect("checked finished above");
            let result = task
                .await
                .map_err(|error| anyhow::anyhow!("daemon task failed: {error}"))?;
            return match result {
                Ok(()) => Err(anyhow::anyhow!("daemon exited before serving IPC")),
                Err(error) => Err(error),
            };
        }
        if ipc_probe(socket).await {
            return Ok(());
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon IPC did not become ready: {}",
            socket.display()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// One `whoami` round trip; false until the daemon serves commands.
async fn ipc_probe(socket: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket).await else {
        return false;
    };
    if stream.write_all(b"{\"cmd\":\"whoami\"}\n").await.is_err() {
        return false;
    }
    let mut line = String::new();
    let read = tokio::time::timeout(
        Duration::from_secs(1),
        BufReader::new(stream).read_line(&mut line),
    )
    .await;
    if !matches!(read, Ok(Ok(n)) if n > 0) {
        return false;
    }
    serde_json::from_str::<Value>(&line).is_ok_and(|reply| reply["ok"] == json!(true))
}

async fn ipc(path: &Path, command: Value) -> Value {
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
    let task = spawn_daemon(root.path(), &paths, &cancel).await;

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
        max_inflight_sends: None,
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
        max_inflight_sends: None,
    })
    .await
    .expect_err("occupied QUIC port must fail startup");

    assert!(error.to_string().contains("bind"));
    assert!(!paths.socket.exists());
    assert!(!root.path().join("daemon.pid").exists());
}

/// A host process can take a probed port between `free_port` and the
/// daemon's UDP bind; startup must retry with a fresh port instead of
/// failing the gate with "failed to bind QUIC endpoint".
#[tokio::test]
async fn daemon_start_retries_a_probed_port_taken_before_bind() {
    let stolen = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let stolen_port = stolen.local_addr().unwrap().port();
    let root = tempdir().unwrap();
    let paths = AxonPaths::from_root(PathBuf::from(root.path()));
    let cancel = CancellationToken::new();

    let mut probed: Vec<u16> = Vec::new();
    let task = {
        let mut next_port = || {
            let port = if probed.is_empty() {
                stolen_port
            } else {
                free_port()
            };
            probed.push(port);
            port
        };
        spawn_daemon_with(root.path(), &paths, &cancel, &mut next_port).await
    };

    assert!(
        probed.len() >= 2,
        "a bound probed port must trigger a retry: {probed:?}"
    );
    // The daemon must have bound the retry port: while it runs, that port is
    // held and a second wildcard bind is refused.
    let retry_port = probed[1];
    assert!(
        std::net::UdpSocket::bind(("0.0.0.0", retry_port)).is_err(),
        "daemon did not bind the retry port {retry_port}"
    );
    let reply = ipc(&paths.socket, json!({"cmd": "whoami"})).await;
    assert_eq!(reply["ok"], true);

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("shutdown timeout")
        .expect("daemon task")
        .expect("daemon result");
    assert!(!paths.socket.exists());
}
