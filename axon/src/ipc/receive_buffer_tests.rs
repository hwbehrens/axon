use super::*;
use crate::message::{Envelope, MessageKind};
use proptest::prelude::*;
use serde_json::json;

fn make_envelope(kind: MessageKind) -> Envelope {
    Envelope::new(
        "ed25519.sender".to_string(),
        "ed25519.receiver".to_string(),
        kind,
        json!({"test": true}),
    )
}

#[test]
fn push_and_fetch_returns_messages_in_order() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    for i in 0..5 {
        let env = Envelope::new(
            format!("ed25519.sender{}", i),
            "ed25519.receiver".to_string(),
            MessageKind::Notify,
            json!({"i": i}),
        );
        buf.push(env);
    }
    let (msgs, has_more) = buf.fetch(10, None, None);
    assert_eq!(msgs.len(), 5);
    assert!(!has_more);
    // Messages should be in insertion order
    for (i, msg) in msgs.iter().enumerate() {
        assert_eq!(
            msg.envelope.from.to_string(),
            format!("ed25519.sender{}", i)
        );
    }
}

#[test]
fn capacity_eviction_drops_oldest() {
    let mut buf = ReceiveBuffer::new(3, 86400);
    for _i in 0..5 {
        buf.push(make_envelope(MessageKind::Notify));
    }
    let (msgs, _) = buf.fetch(10, None, None);
    assert_eq!(msgs.len(), 3);
}

#[test]
fn ack_removes_messages() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    for _ in 0..3 {
        buf.push(make_envelope(MessageKind::Query));
    }
    let (msgs, _) = buf.fetch(10, None, None);
    let ids: Vec<_> = msgs.iter().map(|m| m.envelope.id).collect();
    let acked = buf.ack(&ids[..1]);
    assert_eq!(acked, 1);
    let (remaining, _) = buf.fetch(10, None, None);
    assert_eq!(remaining.len(), 2);
}

#[test]
fn fetch_with_kind_filter() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    buf.push(make_envelope(MessageKind::Query));
    buf.push(make_envelope(MessageKind::Notify));
    buf.push(make_envelope(MessageKind::Query));

    let kinds = [MessageKind::Query];
    let (msgs, _) = buf.fetch(10, None, Some(&kinds));
    assert_eq!(msgs.len(), 2);
}

proptest! {
    #[test]
    fn fetch_limit_clamps_to_1_1000(limit in 0usize..10000) {
        let mut buf = ReceiveBuffer::new(100, 86400);
        for _ in 0..10 {
            buf.push(make_envelope(MessageKind::Notify));
        }
        let (msgs, _) = buf.fetch(limit, None, None);
        let effective_limit = limit.clamp(1, 1000);
        prop_assert!(msgs.len() <= effective_limit);
    }

    #[test]
    fn push_never_exceeds_capacity(capacity in 1usize..50, count in 1usize..100) {
        let mut buf = ReceiveBuffer::new(capacity, 86400);
        for _ in 0..count {
            buf.push(make_envelope(MessageKind::Notify));
        }
        let (msgs, _) = buf.fetch(1000, None, None);
        prop_assert!(msgs.len() <= capacity);
    }

    #[test]
    fn fetch_ordering_preserved(count in 1usize..20) {
        let mut buf = ReceiveBuffer::new(100, 86400);
        for i in 0..count {
            let env = Envelope::new(
                format!("ed25519.sender{}", i),
                "ed25519.receiver".to_string(),
                MessageKind::Notify,
                json!({"i": i}),
            );
            buf.push(env);
        }
        let (msgs, _) = buf.fetch(1000, None, None);
        for (i, msg) in msgs.iter().enumerate() {
            prop_assert_eq!(msg.envelope.from.to_string(), format!("ed25519.sender{}", i));
        }
    }

    #[test]
    fn since_uuid_skips_correctly(n in 2usize..15) {
        let mut buf = ReceiveBuffer::new(100, 86400);
        for _ in 0..n {
            buf.push(make_envelope(MessageKind::Notify));
        }
        let (all_msgs, _) = buf.fetch(1000, None, None);
        // Use the second message's UUID as since
        let since_id = all_msgs[1].envelope.id.to_string();
        let (after_msgs, _) = buf.fetch(1000, Some(&since_id), None);
        // Should get all messages after the second one
        prop_assert_eq!(after_msgs.len(), n - 2);
    }
}
