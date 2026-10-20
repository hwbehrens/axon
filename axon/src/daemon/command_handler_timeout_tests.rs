//! Daemon-level timeout mapping for `send` (spec/IPC.md §5, Q-011).
//!
//! A budget-exhausting exchange must surface as `timeout` — never
//! `peer_unreachable` — and the reply wording must match the phase: a
//! dial-phase timeout names the unreachable locator instead of implying the
//! peer was reached and a response awaited, while a post-write timeout
//! warns that delivery status is unknown. These tests drive in-process
//! daemons over the public IPC surface, proving the dispatch path, the
//! transport deadline, and the reply encoding agree. Split from
//! `command_handler_tests.rs` to stay under the 500-line file limit
//! (CONTRIBUTING.md § "File size").

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

use crate::config::AxonPaths;
use crate::daemon::{DaemonOptions, run_daemon};
use crate::peer_token;

/// Startup attempts before giving up on a probed port that another host
/// process took between `free_port` and the daemon's UDP bind. Each attempt
/// probes a fresh port, so host load cannot fail the run with "failed to
/// bind QUIC endpoint ... Address already in use"; a persistent conflict
/// still fails loudly on the last attempt. Mirrors the retry in
/// `tests/daemon_e2e/ports.rs`.
const PORT_BIND_ATTEMPTS: usize = 3;

/// Bind-and-release a UDP port, remembering allocations so two daemons in
/// this test binary never share one.
fn free_port() -> u16 {
    static CLAIMED: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();
    let claimed = CLAIMED.get_or_init(|| Mutex::new(HashSet::new()));
    loop {
        let port = std::net::UdpSocket::bind("127.0.0.1:0")
            .expect("probe port")
            .local_addr()
            .expect("probe address")
            .port();
        if claimed.lock().expect("port set").insert(port) {
            return port;
        }
    }
}

/// True when a failed daemon start is the probe-to-bind race: an
/// address-in-use bind conflict that a freshly probed port can win.
fn is_port_conflict(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse)
    })
}

struct RunningDaemon {
    paths: AxonPaths,
    port: u16,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    _root: TempDir,
}

impl RunningDaemon {
    async fn spawn() -> Self {
        Self::spawn_with(free_port).await
    }

    /// Spawn and wait until the daemon serves IPC, retrying with a fresh
    /// port when another host process takes the probed port before the
    /// daemon's UDP bind. `next_port` is injectable so the stolen-port
    /// regression can force the race deterministically.
    async fn spawn_with(mut next_port: impl FnMut() -> u16) -> Self {
        let mut attempt = 1;
        loop {
            let mut daemon = start_daemon(next_port());
            match wait_for_ready(&mut daemon).await {
                Ok(()) => return daemon,
                Err(error) if attempt < PORT_BIND_ATTEMPTS && is_port_conflict(&error) => {
                    eprintln!(
                        "[diag] daemon startup hit a port conflict (attempt \
                         {attempt}/{PORT_BIND_ATTEMPTS}): {error:#}; retrying with a fresh port"
                    );
                    attempt += 1;
                }
                Err(error) => {
                    panic!("daemon failed to start after {attempt} attempt(s): {error:#}")
                }
            }
        }
    }

    async fn stop(self) {
        self.cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .expect("daemon shutdown timeout")
            .expect("daemon task")
            .expect("daemon result");
    }
}

/// Start a daemon on `port` without waiting for readiness. A failed bind
/// leaves the task finished, so the caller can classify the error and retry
/// with a fresh port.
fn start_daemon(port: u16) -> RunningDaemon {
    let root = tempfile::tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(root.path()));
    let cancel = CancellationToken::new();
    let task_root = root.path().to_path_buf();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        run_daemon(DaemonOptions {
            port: Some(port),
            disable_mdns: true,
            axon_root: Some(task_root),
            cancel: Some(task_cancel),
            max_inflight_sends: None,
        })
        .await
    });
    RunningDaemon {
        paths,
        port,
        cancel,
        task,
        _root: root,
    }
}

/// Wait until the daemon serves IPC. The `whoami` round trip is handled by
/// the daemon's command loop, which starts only after the QUIC endpoint is
/// bound: unlike raw socket-path existence, this cannot report a daemon
/// that is about to die of a bind failure as ready. On early exit the
/// daemon's own startup error is returned so the caller can classify it.
async fn wait_for_ready(daemon: &mut RunningDaemon) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if daemon.task.is_finished() {
            let result = (&mut daemon.task)
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

async fn ipc_command(socket: &Path, command: Value) -> Value {
    let mut stream = UnixStream::connect(socket).await.expect("ipc connect");
    stream
        .write_all(format!("{command}\n").as_bytes())
        .await
        .expect("ipc write");
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .await
        .expect("ipc read");
    serde_json::from_str(&line).expect("ipc reply is JSON")
}

/// A host process can take a probed port between `free_port` and the
/// daemon's UDP bind; startup must retry with a fresh port instead of
/// failing the run with "failed to bind QUIC endpoint".
#[tokio::test]
async fn stolen_probed_port_is_retried_for_a_single_daemon() {
    let stolen = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind stolen port");
    let stolen_port = stolen.local_addr().expect("stolen address").port();

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
        RunningDaemon::spawn_with(&mut next_port).await
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

/// A send that exhausts its exchange budget must surface as the IPC
/// `timeout` error over the real daemon path, never as `peer_unreachable`,
/// and the post-write response-wait phase must keep its wording plus the
/// duplicate-delivery warning.
///
/// This is a daemon-level regression (two in-process daemons over the public
/// IPC surface) rather than a unit pin on `send_failure`: it proves the
/// dispatch path, the transport deadline, and the reply encoding agree.
#[tokio::test]
async fn timed_out_request_is_reported_as_timeout_not_unreachable() {
    let daemon_a = RunningDaemon::spawn().await;
    let daemon_b = RunningDaemon::spawn().await;

    let info_a = ipc_command(&daemon_a.paths.socket, json!({"cmd": "whoami"})).await;
    let info_b = ipc_command(&daemon_b.paths.socket, json!({"cmd": "whoami"})).await;
    let peer_b = info_b["agent_id"].as_str().expect("peer b id").to_string();
    let key_a = info_a["public_key"].as_str().expect("peer a key");
    let key_b = info_b["public_key"].as_str().expect("peer b key");
    let token_a =
        peer_token::encode(key_a, &format!("127.0.0.1:{}", daemon_a.port)).expect("token a");
    let token_b =
        peer_token::encode(key_b, &format!("127.0.0.1:{}", daemon_b.port)).expect("token b");
    for (socket, token) in [
        (&daemon_a.paths.socket, token_b),
        (&daemon_b.paths.socket, token_a),
    ] {
        let reply = ipc_command(socket, json!({"cmd": "add_peer", "token": token})).await;
        assert_eq!(reply["ok"], true, "enrollment failed: {reply}");
    }

    // B's handler holds the request without replying, so A's response
    // wait (not the dial) exhausts the one-second exchange budget.
    let handler = UnixStream::connect(&daemon_b.paths.socket)
        .await
        .expect("handler connect");
    let (handler_read, mut handler_write) = handler.into_split();
    let mut handler_reader = BufReader::new(handler_read);
    handler_write
        .write_all(b"{\"cmd\":\"serve\"}\n")
        .await
        .expect("serve write");
    let mut ack = String::new();
    handler_reader.read_line(&mut ack).await.expect("serve ack");
    assert_eq!(
        serde_json::from_str::<Value>(&ack).expect("serve reply")["serving"],
        true
    );

    let reply = ipc_command(
        &daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": peer_b,
            "kind": "request",
            "payload": {"question": "never answered"},
            "timeout_secs": 1,
            "req_id": "req-timeout"
        }),
    )
    .await;

    assert_eq!(reply["ok"], false, "unexpected reply: {reply}");
    assert_eq!(
        reply["error"], "timeout",
        "a budget-exhausted send must be reported as timeout, not \
         peer_unreachable: {reply}"
    );
    assert_eq!(reply["req_id"], "req-timeout");
    let message = reply["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("waiting for a response"),
        "the response phase must keep the response-wait wording: {reply}"
    );
    assert!(
        message.contains("delivery status unknown"),
        "a post-write timeout must warn that delivery status is unknown: {reply}"
    );

    drop(handler_write);
    daemon_a.stop().await;
    daemon_b.stop().await;
}

/// A bound-but-silent UDP locator: only the dial budget can end the
/// attempt. Before R-DIAL both kinds came back as `peer_unreachable`
/// although the deadline was exhausted (spec/IPC.md §5, Q-011).
#[tokio::test]
async fn dial_phase_timeout_is_reported_as_timeout_not_unreachable() {
    let daemon = RunningDaemon::spawn().await;
    let sink = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind silent locator");
    let sink_addr = sink.local_addr().expect("sink addr").to_string();
    // A verified identity that runs nowhere; enrollment needs a token.
    let root = tempfile::tempdir().expect("tempdir peer");
    let paths = AxonPaths::from_root(PathBuf::from(root.path()));
    let identity = crate::identity::Identity::load_or_generate(&paths).expect("identity");
    let peer = identity.agent_id().to_string();
    let token = peer_token::encode(identity.public_key_base64(), &sink_addr).expect("token");
    let reply = ipc_command(
        &daemon.paths.socket,
        json!({"cmd": "add_peer", "token": token}),
    )
    .await;
    assert_eq!(reply["ok"], true, "enrollment failed: {reply}");

    // The 1-second request/describe budgets and the fixed 10-second message
    // budget are all spent inside the dial: the kind must not change the
    // `timeout` classification, and the wording must say the peer was never
    // reached instead of claiming a response was awaited.
    for (command, expected) in [
        (
            json!({"cmd": "send", "to": peer, "kind": "request",
                   "payload": {"question": "hello?"}, "timeout_secs": 1,
                   "req_id": "req-dial-timeout"}),
            "request timed out before connecting",
        ),
        (
            json!({"cmd": "send", "to": peer, "kind": "describe",
                   "payload": {"service": "capabilities"}, "timeout_secs": 1,
                   "req_id": "req-dial-describe-timeout"}),
            "describe timed out before connecting",
        ),
        (
            json!({"cmd": "send", "to": peer, "kind": "message",
                   "payload": {"note": "into the void"}}),
            "message send timed out before delivery",
        ),
    ] {
        let reply = ipc_command(&daemon.paths.socket, command).await;
        assert_eq!(
            reply["error"], "timeout",
            "dial budget exhaustion must be timeout, not peer_unreachable: {reply}"
        );
        let message = reply["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(expected),
            "timeout wording must match the kind and phase ({expected}): {reply}"
        );
        if expected != "message send timed out before delivery" {
            // The transport detail names the locator or, when the dial lock
            // was the contended phase, the peer identity. Either proves the
            // dial-phase reason was not discarded.
            assert!(
                message.contains(&sink_addr) || message.contains(&peer),
                "the dial-phase wording must keep the transport's reason \
                 (locator or peer): {reply}"
            );
        }
    }

    drop(sink);
    daemon.stop().await;
}
