//! Send-capacity accounting tests.
//!
//! Pins the P2 contract that `MAX_INFLIGHT_SENDS = N` admits exactly N
//! concurrent sends. The previous scheme incremented the counter before
//! spawning while the handler rejected on `inflight >= max`, so a limit of
//! 1 rejected every send.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::ipc::{IpcErrorCode, IpcSendKind};

use super::{
    MAX_REQUEST_TIMEOUT_SECS, SendSlotGuard, directory_failure, reserve_slot, send_failure,
    send_timeout,
};

#[test]
fn budget_of_one_admits_exactly_one_send() {
    let counter = AtomicUsize::new(0);
    assert!(
        reserve_slot(&counter, 1).is_some(),
        "limit 1 must admit the first send"
    );
    assert_eq!(counter.load(Ordering::Relaxed), 1);
    assert!(
        reserve_slot(&counter, 1).is_none(),
        "second concurrent send must be rejected at limit 1"
    );
    // Release and retry: the slot is reusable.
    counter.fetch_sub(1, Ordering::Relaxed);
    assert!(reserve_slot(&counter, 1).is_some());
}

#[test]
fn exhausted_budget_rejects_until_release() {
    let counter = AtomicUsize::new(0);
    for _ in 0..4 {
        assert!(reserve_slot(&counter, 4).is_some());
    }
    assert!(reserve_slot(&counter, 4).is_none());
    counter.fetch_sub(2, Ordering::Relaxed);
    assert!(reserve_slot(&counter, 4).is_some());
    assert!(reserve_slot(&counter, 4).is_some());
    assert!(reserve_slot(&counter, 4).is_none());
}

#[test]
fn zero_budget_rejects_every_send() {
    let counter = AtomicUsize::new(0);
    assert!(reserve_slot(&counter, 0).is_none());
    assert_eq!(counter.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn racing_reservations_never_exceed_the_budget() {
    use tokio::task::JoinSet;

    const BUDGET: usize = 8;
    const CONTENDERS: usize = 256;
    let counter = Arc::new(AtomicUsize::new(0));
    let mut tasks = JoinSet::new();
    for _ in 0..CONTENDERS {
        let counter = counter.clone();
        tasks.spawn(async move { reserve_slot(&counter, BUDGET).is_some() });
    }
    let mut admitted = 0;
    while let Some(result) = tasks.join_next().await {
        if result.expect("task") {
            admitted += 1;
        }
    }
    assert_eq!(
        admitted, BUDGET,
        "racing reservations must admit exactly the budget"
    );
}

// =========================================================================
// Round-five review regressions: hostile timeout_secs and panic-safe slots.
// =========================================================================

#[test]
fn maximum_timeout_secs_is_rejected_not_overflowed() {
    // `Instant::now() + u64::MAX seconds` panics; the IPC boundary must
    // reject it as an invalid command before any Instant arithmetic.
    let error = send_timeout(IpcSendKind::Request, Some(u64::MAX))
        .expect_err("u64::MAX timeout must be rejected");
    assert_eq!(error.code, IpcErrorCode::InvalidCommand);

    let over = send_timeout(IpcSendKind::Request, Some(MAX_REQUEST_TIMEOUT_SECS + 1))
        .expect_err("timeout above the maximum must be rejected");
    assert_eq!(over.code, IpcErrorCode::InvalidCommand);

    // The boundary itself is accepted.
    assert!(send_timeout(IpcSendKind::Request, Some(MAX_REQUEST_TIMEOUT_SECS)).is_ok());
    assert!(send_timeout(IpcSendKind::Request, None).is_ok());
}

#[test]
fn dropped_send_slot_guard_releases_the_reserved_slot() {
    let counter = Arc::new(AtomicUsize::new(0));
    assert!(reserve_slot(&counter, 1).is_some());
    assert!(reserve_slot(&counter, 1).is_none(), "budget exhausted");

    // The RAII guard releases the slot on drop — including during unwind,
    // so a panicking handler task can no longer leak capacity permanently.
    {
        let _guard = SendSlotGuard(counter.clone());
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }
    assert_eq!(
        counter.load(Ordering::Relaxed),
        0,
        "guard drop must decrement once"
    );
}

// ---------------------------------------------------------------------------
// Round-seven review pin (DEC-022): directory failures map to instructive
// IPC codes — unknown-peer classes stay user-facing, capacity and
// persistence failures are internal errors, never misreported as missing
// peers.
// ---------------------------------------------------------------------------

#[test]
fn directory_failures_map_to_instructive_ipc_codes() {
    use crate::message::AgentId;
    use crate::peer_directory::DirectoryError;

    let agent = AgentId::parse("ed25519.cccccccccccccccccccccccccccccccc").unwrap();

    assert_eq!(
        directory_failure(DirectoryError::NotEnrolled(agent.clone())).code,
        IpcErrorCode::PeerNotFound
    );
    assert_eq!(
        directory_failure(DirectoryError::NotObserved(agent.clone())).code,
        IpcErrorCode::PeerNotObserved
    );
    assert_eq!(
        directory_failure(DirectoryError::LocalAgentId(agent)).code,
        IpcErrorCode::SelfSend
    );
    assert_eq!(
        directory_failure(DirectoryError::EnrolledCapacity).code,
        IpcErrorCode::InternalError,
        "capacity failures must not masquerade as peer_not_found"
    );
    assert_eq!(
        directory_failure(DirectoryError::LocatorCapacity).code,
        IpcErrorCode::InternalError
    );
    assert_eq!(
        directory_failure(DirectoryError::Persist(anyhow::anyhow!("disk on fire"))).code,
        IpcErrorCode::InternalError,
        "persistence failures must not masquerade as peer_not_observed"
    );
}

// ---------------------------------------------------------------------------
// I9: budget exhaustion is `timeout` for EVERY send kind, never
// `peer_unreachable`; delivery ambiguity is surfaced explicitly. The daemon
// path calls `send_failure` for every kind (it no longer inspects `kind`),
// so these mappings are the single chokepoint.
// ---------------------------------------------------------------------------

fn transport_failure(ambiguous: bool, timed_out: bool) -> crate::transport::SendError {
    use crate::transport::SendError;
    match (ambiguous, timed_out) {
        (false, false) => SendError::pre_send(anyhow::anyhow!("dial refused")),
        (true, false) => SendError {
            inner: anyhow::anyhow!("write failed"),
            ambiguous: true,
            timed_out: false,
        },
        (false, true) => SendError::pre_send_timeout(anyhow::anyhow!("dial budget exhausted")),
        (true, true) => SendError {
            inner: anyhow::anyhow!("write budget exhausted"),
            ambiguous: true,
            timed_out: true,
        },
    }
}

#[test]
fn timed_out_transport_failure_is_timeout_for_every_kind() {
    // `message` sends share the transport deadline too; before I9 only
    // request/describe mapped to `timeout` and a message timeout fell
    // through to `peer_unreachable`, violating spec/IPC.md §5.
    for kind in [
        IpcSendKind::Request,
        IpcSendKind::Describe,
        IpcSendKind::Message,
    ] {
        for ambiguous in [false, true] {
            let failure = send_failure(kind, transport_failure(ambiguous, true));
            assert_eq!(
                failure.code,
                IpcErrorCode::Timeout,
                "a timed-out {kind:?} send must never be reported as peer_unreachable"
            );
            if ambiguous {
                assert!(
                    failure.message.contains("delivery status unknown"),
                    "post-delivery budget exhaustion must surface the ambiguity"
                );
            }
        }
    }
}

#[test]
fn timed_out_message_send_does_not_claim_it_waited_for_a_response() {
    // Q-011: `timeout` covers every send kind, but the wording must match the
    // kind. A fire-and-forget send waits for delivery, never for a response.
    let failure = send_failure(IpcSendKind::Message, transport_failure(false, true));
    assert_eq!(failure.code, IpcErrorCode::Timeout);
    assert!(
        failure.message.contains("message send timed out"),
        "unexpected message: {}",
        failure.message
    );
    assert!(
        !failure.message.contains("waiting for a response"),
        "a message send never waits for a response: {}",
        failure.message
    );
}

#[test]
fn ambiguous_non_timeout_failure_surfaces_delivery_ambiguity() {
    let failure = send_failure(IpcSendKind::Message, transport_failure(true, false));
    assert_eq!(failure.code, IpcErrorCode::PeerUnreachable);
    assert!(
        failure.message.contains("delivery status unknown"),
        "an ambiguous failure must warn that a retry could duplicate"
    );
}

#[test]
fn pre_delivery_failure_keeps_the_transport_message() {
    let failure = send_failure(IpcSendKind::Request, transport_failure(false, false));
    assert_eq!(failure.code, IpcErrorCode::PeerUnreachable);
    assert!(failure.message.contains("dial refused"));
    assert!(!failure.message.contains("delivery status unknown"));
}

// =========================================================================
// I9 / Q-011: daemon-level timeout mapping.
// =========================================================================

/// A send that exhausts its exchange budget must surface as the IPC
/// `timeout` error over the real daemon path, never as `peer_unreachable`.
///
/// This is a daemon-level regression (two in-process daemons over the public
/// IPC surface) rather than a unit pin on `send_failure`: it proves the
/// dispatch path, the transport deadline, and the reply encoding agree.
mod timeout_mapping_e2e {
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

    struct RunningDaemon {
        paths: AxonPaths,
        port: u16,
        cancel: CancellationToken,
        task: tokio::task::JoinHandle<anyhow::Result<()>>,
        _root: TempDir,
    }

    impl RunningDaemon {
        async fn spawn() -> Self {
            let root = tempfile::tempdir().expect("tempdir");
            let paths = AxonPaths::from_root(PathBuf::from(root.path()));
            let port = free_port();
            let cancel = CancellationToken::new();
            let task_root = root.path().to_path_buf();
            let task_cancel = cancel.clone();
            let mut task = tokio::spawn(async move {
                run_daemon(DaemonOptions {
                    port: Some(port),
                    disable_mdns: true,
                    axon_root: Some(task_root),
                    cancel: Some(task_cancel),
                    max_inflight_sends: None,
                })
                .await
            });
            wait_for_socket(&paths.socket, &mut task).await;
            Self {
                paths,
                port,
                cancel,
                task,
                _root: root,
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

    /// Wait for the daemon's IPC socket, failing fast with the daemon's own
    /// error when startup fails instead of reporting an opaque timeout.
    async fn wait_for_socket(path: &Path, task: &mut tokio::task::JoinHandle<anyhow::Result<()>>) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !path.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "daemon socket did not appear: {}",
                path.display()
            );
            tokio::select! {
                result = &mut *task => {
                    panic!("daemon exited before its socket appeared: {result:?}");
                }
                _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
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
        assert!(
            reply["message"]
                .as_str()
                .unwrap_or_default()
                .contains("timed out"),
            "the timeout detail must explain the failure: {reply}"
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

        // The 1-second request budget and the fixed 10-second message budget
        // are both spent inside the dial: the kind must not change the
        // `timeout` classification or the kind-aware wording.
        for (command, expected) in [
            (
                json!({"cmd": "send", "to": peer, "kind": "request",
                       "payload": {"question": "hello?"}, "timeout_secs": 1,
                       "req_id": "req-dial-timeout"}),
                "timed out",
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
            assert!(
                reply["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(expected),
                "timeout wording must match the kind ({expected}): {reply}"
            );
        }

        drop(sink);
        daemon.stop().await;
    }
}
