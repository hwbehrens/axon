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
// path calls `send_failure` for every kind, so these mappings are the single
// chokepoint.
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

#[test]
fn pre_send_timeout_keeps_the_transport_detail_without_a_response_claim() {
    // A no-payload-byte timeout (dial lock, resolution, handshake, stream
    // open) must keep the transport's phase detail without claiming a
    // response was awaited.
    for (kind, operation) in [
        (IpcSendKind::Request, "request"),
        (IpcSendKind::Describe, "describe"),
    ] {
        let failure = send_failure(kind, transport_failure(false, true));
        assert_eq!(failure.code, IpcErrorCode::Timeout);
        assert!(
            failure
                .message
                .starts_with(&format!("{operation} timed out before it was sent")),
            "pre-send {operation} wording must not imply a response wait: {}",
            failure.message
        );
        assert!(
            failure.message.contains("dial budget exhausted"),
            "the transport's dial detail must survive: {}",
            failure.message
        );
        assert!(
            !failure.message.contains("waiting for a response"),
            "no payload byte was written, so no response awaited: {}",
            failure.message
        );
    }
}

#[test]
fn post_connect_stream_open_timeout_does_not_claim_the_peer_was_unreached() {
    // The stream-open budget check runs AFTER the peer is connected but
    // BEFORE any payload byte is written: it must stay non-ambiguous and
    // keep the exact phase detail. Before the fix every remaining_budget
    // error was worded "timed out before connecting to the peer", which is
    // false on this path and dropped the request's phase detail.
    for kind in [IpcSendKind::Request, IpcSendKind::Describe] {
        let failure = send_failure(
            kind,
            crate::transport::SendError {
                inner: anyhow::anyhow!("send budget exhausted before bidi stream open"),
                ambiguous: false,
                timed_out: true,
            },
        );
        assert_eq!(failure.code, IpcErrorCode::Timeout);
        assert!(
            failure
                .message
                .contains("send budget exhausted before bidi stream open"),
            "the stream-open phase detail must survive: {}",
            failure.message
        );
        assert!(
            !failure.message.contains("connecting to the peer"),
            "the peer was already connected when the stream open failed: {}",
            failure.message
        );
        assert!(
            !failure.message.contains("waiting for a response"),
            "no request byte was written, so no response was awaited: {}",
            failure.message
        );
        assert!(!failure.message.contains("delivery status unknown"));
    }
}

#[test]
fn post_write_timeout_wording_is_kind_aware() {
    // Budget exhaustion after the request bytes were written is ambiguous:
    // the response-wait wording and the duplicate-delivery warning apply to
    // every kind, and a describe must not borrow the request wording.
    let response_read = || crate::transport::SendError {
        inner: anyhow::anyhow!("send budget exhausted before response read"),
        ambiguous: true,
        timed_out: true,
    };

    let request = send_failure(IpcSendKind::Request, response_read());
    assert_eq!(request.code, IpcErrorCode::Timeout);
    assert!(
        request
            .message
            .starts_with("request timed out waiting for a response;"),
        "{}",
        request.message
    );
    assert!(
        request.message.contains("delivery status unknown"),
        "a post-write timeout must warn that a retry could duplicate: {}",
        request.message
    );

    let describe = send_failure(IpcSendKind::Describe, response_read());
    assert_eq!(describe.code, IpcErrorCode::Timeout);
    assert!(
        describe
            .message
            .starts_with("describe timed out waiting for a response;"),
        "a describe timeout must not reuse the request wording: {}",
        describe.message
    );
    assert!(
        describe.message.contains("delivery status unknown"),
        "a post-write timeout must warn that a retry could duplicate: {}",
        describe.message
    );
}

#[test]
fn post_write_request_timeout_keeps_the_response_wait_and_ambiguity_wording() {
    // The response wait happens after the request bytes were written: the
    // response-phase wording is kept and the retry warning is appended.
    let failure = send_failure(IpcSendKind::Request, transport_failure(true, true));
    assert_eq!(failure.code, IpcErrorCode::Timeout);
    assert!(
        failure.message.contains("waiting for a response"),
        "the response phase must keep its wording: {}",
        failure.message
    );
    assert!(
        failure.message.contains("delivery status unknown"),
        "a post-write timeout must warn about duplicate delivery: {}",
        failure.message
    );
}
