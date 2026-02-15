use super::*;

#[test]
fn reconnect_backoff_doubles_and_caps() {
    let now = Instant::now();
    let mut state = ReconnectState::immediate(now);
    let max_backoff = Duration::from_secs(30);
    assert_eq!(state.current_backoff, Duration::from_secs(1));

    state.schedule_failure(now, max_backoff);
    assert_eq!(state.current_backoff, Duration::from_secs(2));

    state.schedule_failure(now, max_backoff);
    assert_eq!(state.current_backoff, Duration::from_secs(4));

    for _ in 0..10 {
        state.schedule_failure(now, max_backoff);
    }
    assert_eq!(state.current_backoff, Duration::from_secs(30));
}

#[test]
fn reconnect_immediate_is_ready() {
    let now = Instant::now();
    let state = ReconnectState::immediate(now);
    assert!(state.next_attempt_at <= now);
}

#[test]
fn schedule_failure_sets_future_next_attempt() {
    let now = Instant::now();
    let mut state = ReconnectState::immediate(now);
    state.schedule_failure(now, Duration::from_secs(30));
    assert!(
        state.next_attempt_at > now,
        "next_attempt_at must be in the future after schedule_failure"
    );
}
