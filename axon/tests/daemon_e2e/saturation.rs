//! Control-plane liveness under send-capacity saturation.
//!
//! Pins the P1-5 contract: when every send slot is busy, excess sends get
//! an explicit typed rejection and control commands (`status`, `reply`,
//! `serve`, ...) stay responsive. Before this invariant was fixed, the
//! command channel itself was gated on send capacity, letting hung sends
//! block every control operation.
//!
//! Uses a small injected send budget (DaemonOptions::max_inflight_sends)
//! so saturation is deterministic instead of timing-dependent.

use std::path::Path;
use std::time::Duration;

use super::*;

/// A silent handler on B: accepts the request lease and never replies, so
/// every accepted request from A holds its send slot until timeout.
///
/// Returns the connection halves; dropping them would revoke the lease.
async fn install_silent_handler(
    socket: &Path,
) -> (
    BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
) {
    let handler = UnixStream::connect(socket).await.unwrap();
    let (handler_read, mut handler_write) = handler.into_split();
    let mut handler_reader = BufReader::new(handler_read);
    handler_write
        .write_all(b"{\"cmd\":\"serve\"}\n")
        .await
        .unwrap();
    let serving = read_json(&mut handler_reader).await;
    assert_eq!(serving["serving"], json!(true));
    (handler_reader, handler_write)
}

#[tokio::test]
async fn saturated_sends_reject_excess_and_keep_control_responsive() {
    const TINY_BUDGET: usize = 2;
    let port_a = free_port();
    let port_b = free_port();

    let root_a = tempfile::tempdir().unwrap();
    let paths_a = AxonPaths::from_root(root_a.path().to_path_buf());
    paths_a.ensure_root_exists().unwrap();
    let identity_a = Identity::load_or_generate(&paths_a).unwrap();

    let root_b = tempfile::tempdir().unwrap();
    let paths_b = AxonPaths::from_root(PathBuf::from(root_b.path()));
    paths_b.ensure_root_exists().unwrap();
    let identity_b = Identity::load_or_generate(&paths_b).unwrap();

    // Mutual enrollment with known ports before either daemon starts; both
    // daemons load the persisted pins at startup.
    {
        let directory_a = PeerDirectory::load(
            AgentId::parse(identity_a.agent_id()).unwrap(),
            PeerStore::new(paths_a.peers.clone()),
        )
        .await
        .unwrap();
        directory_a
            .enroll(
                PeerIdentity::from_public_key(identity_b.public_key_base64()).unwrap(),
                vec![PeerLocator::Socket(
                    format!("127.0.0.1:{port_b}").parse().unwrap(),
                )],
            )
            .await
            .unwrap();
        let directory_b = PeerDirectory::load(
            AgentId::parse(identity_b.agent_id()).unwrap(),
            PeerStore::new(paths_b.peers.clone()),
        )
        .await
        .unwrap();
        directory_b
            .enroll(
                PeerIdentity::from_public_key(identity_a.public_key_base64()).unwrap(),
                vec![PeerLocator::Socket(
                    format!("127.0.0.1:{port_a}").parse().unwrap(),
                )],
            )
            .await
            .unwrap();
    }

    let spawn_daemon = |port: u16, root: &Path, budget: Option<usize>| {
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let axon_root = root.to_path_buf();
        let task = tokio::spawn(async move {
            run_daemon(DaemonOptions {
                port: Some(port),
                disable_mdns: true,
                axon_root: Some(axon_root),
                cancel: Some(task_cancel),
                max_inflight_sends: budget,
            })
            .await
        });
        (cancel, task)
    };

    let socket_b = paths_b.socket.clone();
    let (cancel_b, task_b) = spawn_daemon(port_b, root_b.path(), None);
    wait_for_socket(&socket_b).await;
    let (cancel_a, task_a) = spawn_daemon(port_a, root_a.path(), Some(TINY_BUDGET));
    let socket_a = paths_a.socket.clone();
    wait_for_socket(&socket_a).await;

    // Holding these open keeps the serve lease alive for the whole test.
    let _handler_conn = install_silent_handler(&socket_b).await;

    // Fill the tiny budget. An accepted request holds its slot silently for
    // the full timeout, so from the probe's perspective "no reply within the
    // window" means the slot was taken; only overflow is answered promptly.
    const HOLD_TIMEOUT_SECS: u64 = 20;
    const PROBE_WINDOW: Duration = Duration::from_secs(3);
    let mut saturated = false;
    let mut slots_taken = 0;
    for index in 0..(TINY_BUDGET * 4) {
        let socket = socket_a.clone();
        let to = identity_b.agent_id().to_string();
        let outcome = tokio::time::timeout(
            PROBE_WINDOW,
            ipc_command(
                &socket,
                json!({
                    "cmd": "send",
                    "to": to,
                    "kind": "request",
                    "payload": {"n": index},
                    "timeout_secs": HOLD_TIMEOUT_SECS
                }),
            ),
        )
        .await;
        match outcome {
            // Slot taken: the request is parked awaiting B's silent handler.
            Err(_) => slots_taken += 1,
            Ok(reply) if reply["error"] == json!("send_capacity_exceeded") => {
                saturated = true;
                eprintln!("[diag] saturated after {slots_taken} held sends");
                break;
            }
            // A transport retry racing cross-dial convergence surfaces as a
            // retryable duplicate/overload reply; keep probing.
            Ok(reply)
                if reply["response"]["payload"]["retryable"] == json!(true)
                    || reply["error"] == json!("send_capacity_exceeded") => {}
            Ok(reply) => panic!("unexpected saturation probe result: {reply}"),
        }
    }
    assert!(
        saturated,
        "expected a send_capacity_exceeded rejection once the budget filled          (held {slots_taken} slots)"
    );

    // Control plane stays responsive at full saturation.
    let started = std::time::Instant::now();
    let status = ipc_command(&socket_a, json!({"cmd": "status"})).await;
    assert_eq!(
        status["ok"],
        json!(true),
        "status must work under saturation"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "control latency under saturation: {:?}",
        started.elapsed()
    );

    // Teardown: cancelling the daemons fails the held sends; their IPC
    // clients observe closed connections, which is expected here.
    cancel_a.cancel();
    cancel_b.cancel();
    tokio::time::timeout(Duration::from_secs(30), async {
        let _ = task_a.await;
        let _ = task_b.await;
    })
    .await
    .expect("daemons shut down");
}
