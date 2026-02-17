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

#[test]
fn immediate_state_is_not_in_flight() {
    let state = ReconnectState::immediate(Instant::now());
    assert!(!state.in_flight, "new state should not be in_flight");
}

#[test]
fn schedule_failure_clears_in_flight() {
    let now = Instant::now();
    let mut state = ReconnectState::immediate(now);
    state.in_flight = true;
    state.schedule_failure(now, Duration::from_secs(30));
    assert!(
        !state.in_flight,
        "schedule_failure must clear in_flight so future ticks can re-attempt"
    );
}

#[tokio::test]
async fn handle_reconnect_outcome_success_removes_state() {
    let peer_table = PeerTable::new();
    let agent_id: AgentId = "ed25519.aabbccdd".into();
    peer_table
        .upsert_discovered(
            agent_id.clone(),
            "127.0.0.1:7100".parse().unwrap(),
            "fakepubkey".to_string(),
        )
        .await;

    let mut reconnect_state = HashMap::new();
    let mut rs = ReconnectState::immediate(Instant::now());
    rs.in_flight = true;
    reconnect_state.insert(agent_id.clone(), rs);

    let outcome = ReconnectOutcome {
        agent_id: agent_id.clone(),
        result: Ok(1.5),
    };
    handle_reconnect_outcome(
        outcome,
        &peer_table,
        &mut reconnect_state,
        Duration::from_secs(30),
    )
    .await;

    assert!(
        !reconnect_state.contains_key(&agent_id),
        "successful outcome should remove reconnect state"
    );
    let peer = peer_table.get(&agent_id).await.unwrap();
    assert_eq!(peer.status, ConnectionStatus::Connected);
}

#[tokio::test]
async fn handle_reconnect_outcome_failure_clears_in_flight_and_backs_off() {
    let peer_table = PeerTable::new();
    let agent_id: AgentId = "ed25519.aabbccdd".into();
    peer_table
        .upsert_discovered(
            agent_id.clone(),
            "127.0.0.1:7100".parse().unwrap(),
            "fakepubkey".to_string(),
        )
        .await;

    let mut reconnect_state = HashMap::new();
    let mut rs = ReconnectState::immediate(Instant::now());
    rs.in_flight = true;
    reconnect_state.insert(agent_id.clone(), rs);

    let outcome = ReconnectOutcome {
        agent_id: agent_id.clone(),
        result: Err(anyhow::anyhow!("connection refused")),
    };
    handle_reconnect_outcome(
        outcome,
        &peer_table,
        &mut reconnect_state,
        Duration::from_secs(30),
    )
    .await;

    let state = reconnect_state
        .get(&agent_id)
        .expect("failure should keep state for retry");
    assert!(!state.in_flight, "failure outcome must clear in_flight");
    assert!(
        state.current_backoff > Duration::from_secs(1),
        "backoff should increase after failure"
    );
    assert!(
        state.next_attempt_at > Instant::now() - Duration::from_millis(100),
        "next_attempt_at should be in the future"
    );
    let peer = peer_table.get(&agent_id).await.unwrap();
    assert_eq!(peer.status, ConnectionStatus::Disconnected);
}
