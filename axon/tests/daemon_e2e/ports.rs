//! Port allocation and daemon readiness.
//!
//! `free_port` probes the OS for an unused UDP port, which leaves a
//! check-then-bind window: another host process can take the probed port
//! before the daemon binds it, and that daemon then dies with "failed to
//! bind QUIC endpoint ... Address already in use" while the test waits for a
//! socket that will never serve. A bounded retry with a freshly probed port
//! closes the window; the regression tests force the race deterministically
//! instead of waiting for host load to win it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axon::config::AxonPaths;
use axon::identity::Identity;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use super::{
    RunningDaemon, ipc_command, prepare_pair_with, spawn_with_budget, wait_for_peer_connected,
};

/// Startup attempts before giving up on a probed port that another host
/// process took between the probe and the daemon's UDP bind. Each attempt
/// probes a fresh port, so a load spike cannot fail the gate with
/// "failed to bind QUIC endpoint", while a persistent conflict still fails
/// loudly on the last attempt.
pub(crate) const PORT_BIND_ATTEMPTS: usize = 3;

pub(crate) fn free_port() -> u16 {
    // Parallel e2e tests each spawn daemons; a naive bind-and-release probe
    // can hand the same port to two tests, making one daemon unbindable or
    // pointing dials at the wrong daemon. Register every handed-out port for
    // the lifetime of the test binary so in-process allocations are unique.
    // Cross-process collisions (the probe-to-bind TOCTOU window) are handled
    // by the startup helpers, which retry on a bind conflict.
    static CLAIMED: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();
    let claimed = CLAIMED.get_or_init(|| Mutex::new(HashSet::new()));
    loop {
        let port = std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        if claimed.lock().unwrap().insert(port) {
            return port;
        }
    }
}

/// True when a failed daemon start is an address-in-use bind conflict: the
/// probe-to-bind race that retrying with a fresh port can win.
pub(crate) fn is_port_conflict(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse)
    })
}

/// Spawn a single daemon and wait until it serves IPC, retrying with a
/// fresh port on a bind conflict.
pub(crate) async fn spawn_ready(
    root: tempfile::TempDir,
    paths: AxonPaths,
    identity: Identity,
) -> RunningDaemon {
    spawn_ready_with(root, paths, identity, free_port).await
}

/// `spawn_ready` with an injectable port source, so the stolen-port
/// regression test can force the probe-to-bind race.
pub(crate) async fn spawn_ready_with(
    root: tempfile::TempDir,
    paths: AxonPaths,
    identity: Identity,
    mut next_port: impl FnMut() -> u16,
) -> RunningDaemon {
    let mut daemon = spawn_with_budget(root, paths, identity, next_port(), None);
    let mut attempt = 1;
    loop {
        match wait_for_ready(&mut daemon).await {
            Ok(()) => return daemon,
            Err(error) if attempt < PORT_BIND_ATTEMPTS && is_port_conflict(&error) => {
                eprintln!(
                    "[diag] daemon startup hit a port conflict (attempt \
                     {attempt}/{PORT_BIND_ATTEMPTS}): {error:#}; retrying with a fresh port"
                );
                let (root, paths, identity) = daemon.into_parts().await;
                attempt += 1;
                daemon = spawn_with_budget(root, paths, identity, next_port(), None);
            }
            Err(error) => panic!("daemon failed to start after {attempt} attempt(s): {error:#}"),
        }
    }
}

/// Wait until the daemon serves IPC. The `whoami` round trip is handled by
/// the daemon's command loop, which starts only after the QUIC endpoint is
/// bound: unlike raw socket-path existence, this cannot report a daemon that
/// is about to die of a bind failure as ready.
pub(crate) async fn wait_for_ready(daemon: &mut RunningDaemon) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if daemon.task.as_ref().is_some_and(|task| task.is_finished()) {
            let task = daemon.task.take().expect("checked finished above");
            let result = task
                .await
                .map_err(|error| anyhow::anyhow!("daemon task failed: {error}"))?;
            return match result {
                Ok(()) => Err(anyhow::anyhow!("daemon exited before serving IPC")),
                Err(error) => Err(error),
            };
        }
        if ipc_probe(&daemon.paths.socket).await {
            return Ok(());
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon IPC did not become ready: {}",
            daemon.paths.socket.display()
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

/// A host process can take a probed port between `free_port` and the
/// daemon's UDP bind; startup must retry with a fresh port instead of
/// failing the gate with "failed to bind QUIC endpoint".
#[tokio::test]
async fn stolen_probed_port_is_retried_for_a_single_daemon() {
    let stolen = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let stolen_port = stolen.local_addr().unwrap().port();

    let root = tempfile::tempdir().unwrap();
    let paths = AxonPaths::from_root(std::path::PathBuf::from(root.path()));
    paths.ensure_root_exists().unwrap();
    let identity = Identity::load_or_generate(&paths).unwrap();

    let mut probed: Vec<u16> = Vec::new();
    let daemon = {
        let mut next_port = || {
            let port = if probed.is_empty() {
                stolen_port
            } else {
                free_port()
            };
            probed.push(port);
            port
        };
        spawn_ready_with(root, paths, identity, &mut next_port).await
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
    let whoami = ipc_command(&daemon.paths.socket, json!({"cmd": "whoami"})).await;
    assert_eq!(whoami["ok"], json!(true), "{whoami}");

    daemon.stop().await;
    assert!(
        std::net::UdpSocket::bind(("0.0.0.0", retry_port)).is_ok(),
        "stopping the daemon must release the retry port {retry_port}"
    );
}

/// The pair path must retry with fresh ports *and* re-enroll: locators
/// persisted for the first attempt would leave the retried daemons dialing
/// a dead port.
#[tokio::test]
async fn stolen_probed_port_is_retried_for_a_daemon_pair() {
    let stolen = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let stolen_port = stolen.local_addr().unwrap().port();

    let mut attempts = 0usize;
    let (daemon_a, daemon_b) = {
        let mut port_a = || {
            attempts += 1;
            if attempts == 1 {
                stolen_port
            } else {
                free_port()
            }
        };
        prepare_pair_with(&mut port_a, free_port, None).await
    };
    assert!(
        attempts >= 2,
        "a bound probed port must trigger a pair retry, attempts={attempts}"
    );

    // Re-enrollment with the retried ports must still connect the pair and
    // carry traffic.
    wait_for_peer_connected(&daemon_a.paths.socket, daemon_b.identity.agent_id()).await;
    let ack = ipc_command(
        &daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": daemon_b.identity.agent_id(),
            "kind": "message",
            "payload": {"after": "port retry"}
        }),
    )
    .await;
    assert_eq!(ack["ok"], json!(true), "{ack}");

    daemon_a.stop().await;
    daemon_b.stop().await;
}
