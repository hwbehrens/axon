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

    // Mutual enrollment with known ports before either daemon starts; both
    // daemons load the persisted pins at startup. The shared pair helper
    // re-enrolls from scratch if a probed port is lost to the probe-to-bind
    // race, so the enrolled locators always name the bound ports.
    let (daemon_a, daemon_b) = prepare_pair_with(free_port, free_port, Some(TINY_BUDGET)).await;
    let socket_a = daemon_a.paths.socket.clone();
    let socket_b = daemon_b.paths.socket.clone();

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
        let to = daemon_b.identity.agent_id().to_string();
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
    tokio::join!(daemon_a.cancel_and_join(), daemon_b.cancel_and_join());
}
