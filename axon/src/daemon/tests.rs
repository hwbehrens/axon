//! Daemon event-loop regression tests (B1 ordering, I8 budget, I13 limiting).

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;

use crate::ipc::{DaemonReply, IpcErrorCode, IpcServer, IpcServerConfig};
use crate::message::AgentId;
use crate::request_broker::RequestBroker;

use super::{
    CANDIDATE_BROADCAST_INTERVAL, MAX_INFLIGHT_WHO_CAN, release_lease_if_client_gone,
    reserve_who_can, should_broadcast_candidate, truncate,
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

/// I13: candidate broadcasts are suppressed inside the rate window and
/// resume on the next window (untrusted candidates cannot starve the queue).
#[test]
fn candidate_broadcasts_are_rate_limited_without_starvation() {
    let t0 = Instant::now();
    let mut last = None;
    assert!(should_broadcast_candidate(&mut last, t0));
    assert!(
        !should_broadcast_candidate(&mut last, t0 + Duration::from_millis(10)),
        "a burst inside the window must be suppressed"
    );
    assert!(
        should_broadcast_candidate(&mut last, t0 + CANDIDATE_BROADCAST_INTERVAL),
        "the next window must broadcast again"
    );
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
