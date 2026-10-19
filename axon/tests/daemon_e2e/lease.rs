//! Handler-lease lifecycle when a client disconnects around `serve`.
//!
//! Regression guard for the blocking finding behind the IPC ownership
//! redesign: a client may send `{"cmd":"serve"}` and close its socket
//! without ever reading the acknowledgement. The IPC disconnect
//! notification can then reach the daemon BEFORE the queued serve command,
//! so the daemon must not leave the lease held by a client that is already
//! gone. If it does, every later `serve` fails with `handler_busy` and
//! inbound requests degrade to `unhandled` forever.

use super::*;

/// Repeatedly close a serve client before its ack; a live client must still
/// be able to take the lease and answer inbound requests.
///
/// Multi-threaded on purpose: the leak is a scheduling race between the
/// daemon's command loop and its IPC cleanup tasks. A single-threaded
/// runtime serializes them and hides the race; the production daemon runs a
/// multi-threaded runtime, so the regression guard must too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serve_lease_is_not_leaked_when_client_closes_before_reading_ack() {
    let (daemon_a, daemon_b) = prepare_pair().await;
    wait_for_peer_connected(&daemon_a.paths.socket, daemon_b.identity.agent_id()).await;

    // Each round queues a serve command and a disconnect notification for
    // the same dead client. Whichever order the daemon observes them in, the
    // lease must end up free: the only failure is observing the disconnect
    // first and then registering the dead client without a liveness
    // recheck. One such round permanently pins the lease, so repetition
    // makes the regression reliable rather than schedule-dependent (the
    // independent review measured ~3% per cycle, with every fresh daemon
    // leaking by 300 cycles).
    for round in 0..300 {
        let mut stream = UnixStream::connect(&daemon_b.paths.socket).await.unwrap();
        stream.write_all(b"{\"cmd\":\"serve\"}\n").await.unwrap();
        drop(stream);
        // Let the server reap the dead client before opening the next one so
        // the 64-client bound is never approached.
        tokio::task::yield_now().await;
        if round % 64 == 63 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    // Let the daemon drain both queues before asserting.
    tokio::time::sleep(Duration::from_millis(250)).await;

    let handler_stream = UnixStream::connect(&daemon_b.paths.socket).await.unwrap();
    let (handler_read, mut handler_write) = handler_stream.into_split();
    let mut handler_reader = BufReader::new(handler_read);
    handler_write
        .write_all(b"{\"cmd\":\"serve\"}\n")
        .await
        .unwrap();
    let serving = read_json(&mut handler_reader).await;
    assert_eq!(
        serving["ok"],
        json!(true),
        "handler lease leaked by a client that closed before its serve ack: {serving}"
    );
    assert_eq!(serving["serving"], json!(true));

    // The lease must also be functional: the live handler receives inbound
    // requests and can answer them.
    let socket_a = daemon_a.paths.socket.clone();
    let peer_b = daemon_b.identity.agent_id().to_string();
    let outbound = tokio::spawn(async move {
        ipc_command(
            &socket_a,
            json!({
                "cmd": "send",
                "to": peer_b,
                "kind": "request",
                "payload": {"question": "still alive?"},
                "timeout_secs": 5
            }),
        )
        .await
    });
    let request = read_json(&mut handler_reader).await;
    assert_eq!(request["event"], "request");
    handler_write
        .write_all(
            format!(
                "{}\n",
                json!({
                    "cmd": "reply",
                    "request_id": request["request_id"],
                    "kind": "response",
                    "payload": {"answer": "yes"}
                })
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    assert_eq!(read_json(&mut handler_reader).await["ok"], json!(true));
    let response = outbound.await.unwrap();
    assert_eq!(response["ok"], json!(true));
    assert_eq!(response["response"]["payload"]["answer"], "yes");

    daemon_a.stop().await;
    daemon_b.stop().await;
}
