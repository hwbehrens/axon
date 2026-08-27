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
    MAX_REQUEST_TIMEOUT_SECS, SendSlotGuard, directory_failure, reserve_slot, send_timeout,
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
