//! Send-capacity accounting tests.
//!
//! Pins the P2 contract that `MAX_INFLIGHT_SENDS = N` admits exactly N
//! concurrent sends. The previous scheme incremented the counter before
//! spawning while the handler rejected on `inflight >= max`, so a limit of
//! 1 rejected every send.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::reserve_slot;

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
