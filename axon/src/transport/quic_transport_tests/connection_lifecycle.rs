use std::time::{Duration, Instant};

use serde_json::json;

use super::fixtures::make_transport_pair;
use crate::message::{AgentId, Envelope, MessageKind};

#[tokio::test]
async fn simultaneous_cross_dial_converges_to_one_connection() {
    let pair = make_transport_pair().await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let message_a = Envelope::new(
        agent_a.clone(),
        agent_b.clone(),
        MessageKind::Message,
        json!({"from": "a"}),
    );
    let message_b = Envelope::new(
        agent_b.clone(),
        agent_a.clone(),
        MessageKind::Message,
        json!({"from": "b"}),
    );

    let (sent_a, sent_b) = tokio::join!(
        pair.transport_a.send_to(
            &pair.directory_a,
            &agent_b,
            message_a,
            Duration::from_secs(5),
        ),
        pair.transport_b.send_to(
            &pair.directory_b,
            &agent_a,
            message_b,
            Duration::from_secs(5),
        ),
    );
    sent_a.expect("A to B");
    sent_b.expect("B to A");

    let deadline = Instant::now() + Duration::from_secs(5);
    while (pair.transport_a.connected_count().await != 1
        || pair.transport_b.connected_count().await != 1)
        && Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(pair.transport_a.connected_count().await, 1);
    assert_eq!(pair.transport_b.connected_count().await, 1);
}

#[tokio::test]
async fn explicit_refresh_advances_slot_and_reconnects() {
    let pair = make_transport_pair().await;
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let first = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::Message,
        json!({"attempt": 1}),
    );
    pair.transport_a
        .send_to(&pair.directory_a, &agent_b, first, Duration::from_secs(5))
        .await
        .expect("first send");
    pair.transport_a.close_peer(&agent_b, b"test refresh").await;
    assert!(!pair.transport_a.has_connection(&agent_b).await);

    let second = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        AgentId::parse(pair.id_b.agent_id()).unwrap(),
        MessageKind::Message,
        json!({"attempt": 2}),
    );
    pair.transport_a
        .send_to(&pair.directory_a, &agent_b, second, Duration::from_secs(5))
        .await
        .expect("send after refresh");
    assert!(pair.transport_a.has_connection(&agent_b).await);
}
