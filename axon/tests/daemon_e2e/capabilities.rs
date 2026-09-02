//! Full-stack capability-manifest behavior across a real daemon pair.
//!
//! Pins the describe contract end to end: `serve` publishes a manifest with
//! the handler lease, the receiving daemon answers `describe` without waking
//! the handler, `who_can` is a cached derived view, and `peers` surfaces an
//! advisory service summary.

use super::*;

fn sample_manifest_command() -> Value {
    json!({
        "cmd": "serve",
        "manifest": {
            "name": "forge",
            "services": [{
                "id": "cargo_test",
                "description": "Run cargo test on a workspace.",
                "example_request": {"workspace": "/srv/axon"},
                "timeout_hint_secs": 900
            }]
        }
    })
}

/// A `describe` to a peer with no published manifest must answer with an
/// explicit, instructive error — never silence, never handler wake-up.
#[tokio::test]
async fn describe_without_manifest_answers_no_manifest_error() {
    let (daemon_a, daemon_b) = prepare_pair().await;

    let ack = ipc_command(
        &daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": daemon_b.identity.agent_id(),
            "kind": "describe",
            "payload": {},
            "timeout_secs": 5
        }),
    )
    .await;
    assert_eq!(ack["ok"], json!(true), "the send itself must succeed");
    assert_eq!(ack["response"]["kind"], "error");
    assert_eq!(ack["response"]["payload"]["code"], "no_manifest");
    assert_eq!(ack["response"]["payload"]["retryable"], json!(false));
    assert!(
        ack["response"]["payload"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("serve with a manifest")),
        "the error must teach the corrective action"
    );

    daemon_a.stop().await;
    daemon_b.stop().await;
}

/// Serving a manifest publishes it: remote `describe` returns it, `who_can`
/// finds it via a pull, and `peers` lists the advisory service summary.
#[tokio::test]
async fn served_manifest_is_describable_queryable_and_summarized() {
    let (daemon_a, daemon_b) = prepare_pair().await;
    wait_for_peer_connected(&daemon_a.paths.socket, daemon_b.identity.agent_id()).await;

    // A persistent IPC client on B holds the handler lease and publishes the
    // manifest; the lease (and manifest) live for the connection lifetime.
    let handler_stream = UnixStream::connect(&daemon_b.paths.socket).await.unwrap();
    let (handler_read, mut handler_write) = handler_stream.into_split();
    let mut handler_reader = BufReader::new(handler_read);
    handler_write
        .write_all(format!("{}\n", sample_manifest_command()).as_bytes())
        .await
        .unwrap();
    let serving = read_json(&mut handler_reader).await;
    assert_eq!(serving["ok"], json!(true), "serve must succeed");
    assert_eq!(serving["serving"], json!(true));

    // Direct describe over the send path: answered by B's daemon from the
    // registered manifest, inline in A's send result.
    let ack = ipc_command(
        &daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": daemon_b.identity.agent_id(),
            "kind": "describe",
            "payload": {},
            "timeout_secs": 5
        }),
    )
    .await;
    assert_eq!(ack["ok"], json!(true));
    assert_eq!(ack["response"]["kind"], "response");
    assert_eq!(ack["response"]["payload"]["name"], "forge");
    assert_eq!(
        ack["response"]["payload"]["services"][0]["id"],
        "cargo_test"
    );

    // who_can: A's daemon pulls B's manifest over QUIC and caches it.
    let reply = ipc_command(
        &daemon_a.paths.socket,
        json!({"cmd": "who_can", "query": "cargo"}),
    )
    .await;
    assert_eq!(reply["ok"], json!(true));
    assert_eq!(reply["unreachable"], json!([]));
    assert_eq!(reply["matches"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        reply["matches"][0]["agent_id"],
        json!(daemon_b.identity.agent_id())
    );
    assert_eq!(reply["matches"][0]["services"][0]["id"], "cargo_test");

    // A case-insensitive miss returns an empty match set, not an error.
    let reply = ipc_command(
        &daemon_a.paths.socket,
        json!({"cmd": "who_can", "query": "docker"}),
    )
    .await;
    assert_eq!(reply["ok"], json!(true));
    assert_eq!(reply["matches"], json!([]));

    // peers: the cached manifest surfaces as an advisory services summary.
    let reply = ipc_command(&daemon_a.paths.socket, json!({"cmd": "peers"})).await;
    let peer = reply["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["agent_id"] == json!(daemon_b.identity.agent_id()))
        .expect("peer B listed")
        .clone();
    assert_eq!(peer["services"], json!(["cargo_test"]));

    daemon_a.stop().await;
    daemon_b.stop().await;
}

/// A second `serve` from the same lease holder refreshes the published
/// manifest; `describe` answers with the new content immediately.
#[tokio::test]
async fn re_serving_refreshes_the_published_manifest() {
    let (daemon_a, daemon_b) = prepare_pair().await;

    let handler_stream = UnixStream::connect(&daemon_b.paths.socket).await.unwrap();
    let (handler_read, mut handler_write) = handler_stream.into_split();
    let mut handler_reader = BufReader::new(handler_read);
    handler_write
        .write_all(format!("{}\n", sample_manifest_command()).as_bytes())
        .await
        .unwrap();
    let serving = read_json(&mut handler_reader).await;
    assert_eq!(serving["ok"], json!(true));

    let refreshed = json!({
        "cmd": "serve",
        "manifest": {
            "name": "forge-v2",
            "services": [{"id": "lint", "description": "Run clippy."}]
        }
    });
    handler_write
        .write_all(format!("{refreshed}\n").as_bytes())
        .await
        .unwrap();
    let serving = read_json(&mut handler_reader).await;
    assert_eq!(
        serving["ok"],
        json!(true),
        "idempotent re-serve must succeed"
    );

    let ack = ipc_command(
        &daemon_a.paths.socket,
        json!({
            "cmd": "send",
            "to": daemon_b.identity.agent_id(),
            "kind": "describe",
            "payload": {},
            "timeout_secs": 5
        }),
    )
    .await;
    assert_eq!(
        ack["response"]["payload"]["name"], "forge-v2",
        "describe must answer with the refreshed manifest"
    );
    assert_eq!(ack["response"]["payload"]["services"][0]["id"], "lint");

    daemon_a.stop().await;
    daemon_b.stop().await;
}
