//! Daemon event-loop regression tests (B1 ordering, I8 budget, I13 limiting).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::sync::Semaphore;

use crate::ipc::{DaemonReply, IpcErrorCode, IpcServer, IpcServerConfig, encode_reply_line};
use crate::message::{AgentId, Envelope, MAX_MESSAGE_SIZE, MessageKind};
use crate::request_broker::RequestBroker;
use crate::transport::{ConnectionManager, InboundReceiver};

use super::{
    CANDIDATE_BROADCAST_INTERVAL, CandidateBroadcast, MAX_INFLIGHT_WHO_CAN, PendingCandidate,
    flush_candidates, make_response_handler, reconcile_after_lag, release_lease_if_client_gone,
    reserve_who_can, subscribe_inbound_counted, truncate,
};

fn local_id() -> AgentId {
    AgentId::parse("ed25519.cccccccccccccccccccccccccccccccc").unwrap()
}

/// B1: a `serve` queued before its client disconnected must not leak the
/// handler lease when the disconnect notification is processed first.
#[tokio::test]
async fn serve_for_disconnected_client_releases_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let (ipc, _commands) =
        IpcServer::bind(dir.path().join("axon.sock"), 4, IpcServerConfig::default())
            .await
            .unwrap();
    let broker = RequestBroker::new(local_id());
    assert!(broker.register(1, None).await.is_ok());

    release_lease_if_client_gone(&ipc, &broker, 1).await;

    assert!(
        broker.register(2, None).await.is_ok(),
        "lease of a disconnected serve client must be released"
    );
    ipc.shutdown().await.unwrap();
}

/// B1 counterpart: a live client's lease must survive the re-check.
#[tokio::test]
async fn serve_for_connected_client_keeps_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("axon.sock");
    let (ipc, _commands) = IpcServer::bind(socket.clone(), 4, IpcServerConfig::default())
        .await
        .unwrap();
    let broker = RequestBroker::new(local_id());
    let _client = tokio::net::UnixStream::connect(&socket).await.unwrap();
    let registered = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if ipc.connected_client_ids().await.contains(&1) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        registered.is_ok(),
        "client 1 must register before the check"
    );

    assert!(broker.register(1, None).await.is_ok());
    release_lease_if_client_gone(&ipc, &broker, 1).await;

    assert!(
        broker.register(2, None).await.is_err(),
        "a live client's lease must not be revoked by the re-check"
    );
    ipc.shutdown().await.unwrap();
}

/// A wire-legal near-limit request wraps into an oversized IPC request
/// event. That encoding failure must fail only the affected request: the
/// still-connected handler keeps its lease (spec/IPC.md §2, §6.2).
#[tokio::test]
async fn oversized_request_event_keeps_the_live_handler_lease() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("axon.sock");
    let (ipc, _commands) = IpcServer::bind(socket.clone(), 4, IpcServerConfig::default())
        .await
        .unwrap();
    let _client = tokio::net::UnixStream::connect(&socket).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !ipc.connected_client_ids().await.contains(&1) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("client 1 must connect before the lease is registered");

    let broker = RequestBroker::new(local_id());
    broker.register(1, None).await.expect("handler lease");
    let handler = make_response_handler(ipc.clone(), broker.clone());

    // `wire_encode` strips `from`/`to`, so a request can be legal on the
    // QUIC wire up to exactly MAX_MESSAGE_SIZE and still overflow the IPC
    // request event, which re-adds both fields plus the event wrapper.
    let remote = AgentId::parse("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    let base = Envelope::new(
        remote.clone(),
        local_id(),
        MessageKind::Request,
        json!({"pad": ""}),
    );
    let pad = MAX_MESSAGE_SIZE as usize - base.wire_encode().unwrap().len();
    let request = Arc::new(Envelope::new(
        remote,
        local_id(),
        MessageKind::Request,
        json!({"pad": "x".repeat(pad)}),
    ));
    assert_eq!(
        request.wire_encode().unwrap().len(),
        MAX_MESSAGE_SIZE as usize,
        "the fixture request must be wire-legal"
    );
    let event = DaemonReply::RequestEvent {
        event: "request",
        request_id: request.id,
        from: request.from.as_ref().unwrap().to_string(),
        envelope: (*request).clone(),
    };
    assert!(
        encode_reply_line(&event).is_err(),
        "the fixture request event must exceed the IPC line limit"
    );

    let response = tokio::time::timeout(Duration::from_secs(5), handler(request.clone()))
        .await
        .expect("delivery failure must not wait for the request deadline")
        .expect("the response handler always answers");
    assert_eq!(response.kind, MessageKind::Error);
    let payload = response.payload_value().expect("error payload");
    assert_eq!(payload["code"], json!("overloaded"));
    assert_eq!(payload["retryable"], json!(true));

    assert!(
        broker.register(2, None).await.is_err(),
        "an oversized delivery must not revoke a live handler's lease"
    );
    assert!(
        ipc.connected_client_ids().await.contains(&1),
        "the handler client must remain connected"
    );
    ipc.shutdown().await.unwrap();
}

/// I16 consumer: reconciliation after a lagged broadcast must release the
/// lease of a handler that is gone while keeping a live handler's lease, so
/// lost disconnect notifications cannot pin it.
#[tokio::test]
async fn lag_reconcile_keeps_live_lease_and_releases_gone_lease() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("axon.sock");
    let (ipc, _commands) = IpcServer::bind(socket.clone(), 4, IpcServerConfig::default())
        .await
        .unwrap();
    let broker = RequestBroker::new(local_id());
    let client = tokio::net::UnixStream::connect(&socket).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !ipc.connected_client_ids().await.contains(&1) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("client 1 must connect");

    broker.register(1, None).await.expect("handler lease");
    reconcile_after_lag(&ipc, &broker).await;
    assert!(
        broker.register(2, None).await.is_err(),
        "a live handler's lease must survive lag reconciliation"
    );

    drop(client);
    tokio::time::timeout(Duration::from_secs(2), async {
        while ipc.connected_client_ids().await.contains(&1) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("client 1 must disconnect");
    // No daemon loop runs here, so only the lag reconcile can release it.
    reconcile_after_lag(&ipc, &broker).await;
    assert!(
        broker.register(2, None).await.is_ok(),
        "a gone handler's lease must be released by lag reconciliation"
    );
    ipc.shutdown().await.unwrap();
}

/// I8: the `who_can` budget saturates at N and overflow is a typed,
/// request-correlated error; released slots become reusable.
#[tokio::test]
async fn who_can_budget_rejects_overflow_with_typed_backpressure() {
    let budget = Arc::new(Semaphore::new(MAX_INFLIGHT_WHO_CAN));
    let permits: Vec<_> = (0..MAX_INFLIGHT_WHO_CAN)
        .map(|_| reserve_who_can(&budget, None).expect("within budget"))
        .collect();
    let reply = reserve_who_can(&budget, Some("req-1".into())).expect_err("overflow");
    match *reply {
        DaemonReply::Error {
            ok, error, req_id, ..
        } => {
            assert!(!ok);
            assert_eq!(error, IpcErrorCode::SendCapacityExceeded);
            assert_eq!(req_id.as_deref(), Some("req-1"));
        }
        other => panic!("expected typed error reply, got {other:?}"),
    }
    drop(permits);
    assert!(
        reserve_who_can(&budget, None).is_ok(),
        "released slots must be reusable"
    );
}

/// I13/R1: candidate broadcasts stay rate-limited, but the burst inside the
/// window coalesces instead of being dropped.
#[test]
fn candidate_broadcasts_are_rate_limited_without_starvation() {
    let t0 = Instant::now();
    let mut broadcast = CandidateBroadcast::default();
    assert!(
        broadcast.enqueue(queued_candidate(0), t0),
        "the first candidate opens the window"
    );
    let _ = take_pending(&mut broadcast);
    assert!(
        !broadcast.enqueue(queued_candidate(1), t0 + Duration::from_millis(10)),
        "a burst inside the window must be queued, not broadcast"
    );
    assert_eq!(
        take_pending(&mut broadcast).len(),
        1,
        "the queued candidate must survive the window"
    );
    assert!(
        broadcast.enqueue(queued_candidate(2), t0 + CANDIDATE_BROADCAST_INTERVAL),
        "the next window must broadcast again"
    );
}

/// R1: a burst larger than one rate window must eventually deliver every
/// distinct candidate. The old limiter dropped each event inside the window
/// and only ever released the burst's first candidate, so an insertion-
/// ordered refresh replay permanently starved the candidates behind it.
#[test]
fn candidate_burst_coalesces_and_never_loses_a_distinct_candidate() {
    let t0 = Instant::now();
    let mut broadcast = CandidateBroadcast::default();
    let mut released = Vec::new();

    for index in 0..64u16 {
        if broadcast.enqueue(queued_candidate(index), t0) {
            released.extend(take_pending(&mut broadcast));
        }
    }
    assert_eq!(
        released.len(),
        1,
        "only the first candidate opens the window"
    );

    // The daemon's flush branch releases the queue when the window opens.
    released.extend(take_pending(&mut broadcast));
    let distinct: std::collections::BTreeSet<_> = released.into_iter().collect();
    assert_eq!(distinct.len(), 64, "no distinct candidate may be starved");
}

/// R1: the queue keeps the latest state per Agent ID; a refresh replaces the
/// queued record instead of duplicating or dropping it.
#[test]
fn candidate_queue_keeps_only_the_latest_state_per_agent() {
    let t0 = Instant::now();
    let mut broadcast = CandidateBroadcast::default();
    assert!(broadcast.enqueue(queued_candidate(7), t0));
    let _ = take_pending(&mut broadcast);
    assert!(!broadcast.enqueue(
        queued_candidate_at(7, "127.0.0.1:9999"),
        t0 + Duration::from_millis(10)
    ));
    let locators: Vec<_> = std::mem::take(&mut broadcast.pending)
        .into_values()
        .map(|candidate| candidate.2)
        .collect();
    assert_eq!(locators, vec![vec!["127.0.0.1:9999".to_string()]]);
}

/// R1 delivery half: releasing the coalesced queue broadcasts every queued
/// candidate to connected IPC clients.
#[tokio::test]
async fn coalesced_candidates_reach_connected_clients() {
    const QUEUED: u16 = 8;
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("axon.sock");
    let (ipc, _commands) = IpcServer::bind(socket.clone(), 4, IpcServerConfig::default())
        .await
        .unwrap();
    let client = tokio::net::UnixStream::connect(&socket).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !ipc.connected_client_ids().await.contains(&1) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("client 1 must connect");

    let mut broadcast = CandidateBroadcast::default();
    let t0 = Instant::now();
    for index in 0..QUEUED {
        broadcast.enqueue(queued_candidate(index), t0);
    }
    let queued = std::mem::take(&mut broadcast.pending);
    flush_candidates(&ipc, queued.into_values()).await;

    let mut reader = tokio::io::BufReader::new(client);
    let mut seen = std::collections::BTreeSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while (seen.len() as u16) < QUEUED {
        let mut line = String::new();
        let read = tokio::time::timeout_at(deadline, reader.read_line(&mut line)).await;
        assert!(read.is_ok(), "timed out waiting for candidates: {seen:?}");
        let value: serde_json::Value = serde_json::from_str(line.trim()).expect("event JSON");
        if value["event"] == "peer_candidate"
            && let Some(agent_id) = value["agent_id"].as_str()
        {
            seen.insert(agent_id.to_string());
        }
    }
    assert_eq!(seen.len(), QUEUED as usize);
    ipc.shutdown().await.unwrap();
}

/// R2 guard: the daemon must subscribe through the counted receiver, so
/// broadcast evictions reach `inbound_gap_count` and the Lagged warning.
/// The runtime gap accounting itself is pinned by the transport test
/// `counted_subscription_records_broadcast_gaps`.
#[test]
fn daemon_subscribes_with_inbound_gap_accounting() {
    let _: fn(&ConnectionManager) -> InboundReceiver = subscribe_inbound_counted;
}

fn queued_candidate(index: u16) -> PendingCandidate {
    queued_candidate_at(index, &format!("127.0.0.1:{}", 7100 + index))
}

fn queued_candidate_at(index: u16, locator: &str) -> PendingCandidate {
    let agent_id = AgentId::parse(&format!("ed25519.{index:032x}")).unwrap();
    (
        agent_id,
        "cHVia2V5".to_string(),
        vec![locator.to_string()],
        "mdns",
    )
}

fn take_pending(broadcast: &mut CandidateBroadcast) -> Vec<String> {
    std::mem::take(&mut broadcast.pending)
        .into_keys()
        .map(|agent_id| agent_id.to_string())
        .collect()
}

/// Nit: the attacker-chosen message-kind log field is bounded to 64 bytes.
#[test]
fn message_kind_log_field_is_bounded() {
    let long = "x".repeat(4096);
    let bounded = truncate(&long, 64);
    assert_eq!(bounded.chars().count(), 65, "64 bytes plus the ellipsis");
    assert!(bounded.ends_with('…'));
    assert_eq!(truncate("message", 64), "message");
}
