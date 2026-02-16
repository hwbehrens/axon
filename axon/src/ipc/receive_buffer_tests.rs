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
    let (msgs, next_seq, has_more) = buf.fetch("c1", 10, None);
    assert_eq!(msgs.len(), 5);
    assert!(!has_more);
    assert_eq!(next_seq, Some(5));
    for (i, msg) in msgs.iter().enumerate() {
        assert_eq!(msg.seq, (i + 1) as u64);
        assert_eq!(
            msg.envelope.from.to_string(),
            format!("ed25519.sender{}", i)
        );
    }
}

#[test]
fn capacity_eviction_drops_oldest() {
    let mut buf = ReceiveBuffer::new(3, 86400);
    for _ in 0..5 {
        buf.push(make_envelope(MessageKind::Notify));
    }
    let (msgs, _, _) = buf.fetch("c1", 10, None);
    assert_eq!(msgs.len(), 3);
    // Oldest two (seq 1,2) were evicted; remaining are seq 3,4,5
    assert_eq!(msgs[0].seq, 3);
    assert_eq!(msgs[2].seq, 5);
}

#[test]
fn ack_advances_cursor_without_deleting() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    for _ in 0..3 {
        buf.push(make_envelope(MessageKind::Query));
    }
    let (msgs, _, _) = buf.fetch("c1", 10, None);
    assert_eq!(msgs.len(), 3);

    // Mark delivered before acking
    buf.update_delivered_seq("c1", msgs.last().unwrap().seq);

    // Ack up to the first message's seq
    let first_seq = msgs[0].seq;
    let result = buf.ack("c1", first_seq);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), first_seq);

    // Subsequent fetch should skip the acked message
    let (remaining, _, _) = buf.fetch("c1", 10, None);
    assert_eq!(remaining.len(), 2);
    assert_eq!(remaining[0].seq, first_seq + 1);
}

#[test]
fn ack_out_of_range_rejected() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    for _ in 0..3 {
        buf.push(make_envelope(MessageKind::Notify));
    }

    // Fetch only 2 (so highest_delivered_seq = 2)
    let (msgs, _, has_more) = buf.fetch("c1", 2, None);
    assert_eq!(msgs.len(), 2);
    assert!(has_more);

    // Mark delivered for the fetched messages
    buf.update_delivered_seq("c1", 2);

    // Try to ack beyond highest delivered → OutOfRange
    let result = buf.ack("c1", 3);
    assert_eq!(result, Err(AckError::OutOfRange));

    // Ack within range should succeed
    let result = buf.ack("c1", 2);
    assert!(result.is_ok());
}

#[test]
fn fetch_with_kind_filter_stops_at_non_matching() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    buf.push(make_envelope(MessageKind::Query));
    buf.push(make_envelope(MessageKind::Notify));
    buf.push(make_envelope(MessageKind::Query));

    let kinds = [MessageKind::Query];
    let (msgs, next_seq, has_more) = buf.fetch("c1", 10, Some(&kinds));
    // Stops at seq=2 (Notify doesn't match), returns only seq=1
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].envelope.kind, MessageKind::Query);
    assert_eq!(next_seq, Some(1));
    assert!(has_more, "has_more because non-matching message blocks");
}

#[test]
fn fetch_with_kind_filter_contiguous_matching() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    buf.push(make_envelope(MessageKind::Query));
    buf.push(make_envelope(MessageKind::Query));
    buf.push(make_envelope(MessageKind::Notify));

    let kinds = [MessageKind::Query];
    let (msgs, next_seq, has_more) = buf.fetch("c1", 10, Some(&kinds));
    // Returns both contiguous Query messages, stops at Notify
    assert_eq!(msgs.len(), 2);
    assert_eq!(next_seq, Some(2));
    assert!(has_more);
}

#[test]
fn fetch_without_filter_returns_all() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    buf.push(make_envelope(MessageKind::Query));
    buf.push(make_envelope(MessageKind::Notify));
    buf.push(make_envelope(MessageKind::Query));

    let (msgs, _, has_more) = buf.fetch("c1", 10, None);
    assert_eq!(msgs.len(), 3);
    assert!(!has_more);
}

#[test]
fn multi_consumer_independence() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    for _ in 0..3 {
        buf.push(make_envelope(MessageKind::Notify));
    }

    // Consumer A fetches and acks first two
    let (msgs_a, _, _) = buf.fetch("a", 10, None);
    assert_eq!(msgs_a.len(), 3);
    buf.update_delivered_seq("a", 3);
    buf.ack("a", 2).unwrap();

    // Consumer B has not fetched yet — sees all 3
    let (msgs_b, _, _) = buf.fetch("b", 10, None);
    assert_eq!(msgs_b.len(), 3);
    buf.update_delivered_seq("b", 3);

    // Consumer A now sees only 1 remaining
    let (msgs_a2, _, _) = buf.fetch("a", 10, None);
    assert_eq!(msgs_a2.len(), 1);
    assert_eq!(msgs_a2[0].seq, 3);

    // Consumer B acks all 3
    buf.ack("b", 3).unwrap();
    let (msgs_b2, _, _) = buf.fetch("b", 10, None);
    assert_eq!(msgs_b2.len(), 0);
}

#[test]
fn byte_cap_eviction() {
    // Create a buffer with a very small byte cap
    let mut buf = ReceiveBuffer::new(100, 86400).with_byte_cap(200);

    // Push messages that together exceed the byte cap
    for _ in 0..5 {
        buf.push(make_envelope(MessageKind::Notify));
    }

    let (msgs, _, _) = buf.fetch("c1", 100, None);
    // Byte cap should have evicted some of the oldest messages
    assert!(msgs.len() < 5);
    assert!(!msgs.is_empty());
}

#[test]
fn seq_monotonically_increases() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    let mut seqs = Vec::new();
    for _ in 0..5 {
        let (seq, _) = buf.push(make_envelope(MessageKind::Notify));
        seqs.push(seq);
    }
    for window in seqs.windows(2) {
        assert!(window[1] > window[0], "seq must strictly increase");
    }
    assert_eq!(seqs, vec![1, 2, 3, 4, 5]);
}

#[test]
fn fetch_respects_acked_seq() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    for _ in 0..5 {
        buf.push(make_envelope(MessageKind::Notify));
    }

    // Fetch all, then ack first 3
    let (msgs, _, _) = buf.fetch("c1", 10, None);
    assert_eq!(msgs.len(), 5);
    buf.update_delivered_seq("c1", 5);
    buf.ack("c1", 3).unwrap();

    // Next fetch should only return seq 4, 5
    let (msgs2, next_seq, has_more) = buf.fetch("c1", 10, None);
    assert_eq!(msgs2.len(), 2);
    assert_eq!(msgs2[0].seq, 4);
    assert_eq!(msgs2[1].seq, 5);
    assert_eq!(next_seq, Some(5));
    assert!(!has_more);
}

#[test]
fn buffer_size_zero_disables_buffering() {
    let mut buf = ReceiveBuffer::new(0, 86400);
    let (seq1, _) = buf.push(make_envelope(MessageKind::Notify));
    let (seq2, _) = buf.push(make_envelope(MessageKind::Query));

    // Seqs still increment
    assert_eq!(seq1, 1);
    assert_eq!(seq2, 2);

    // But inbox is always empty
    let (msgs, _, _) = buf.fetch("c1", 10, None);
    assert_eq!(msgs.len(), 0);

    // highest_seq returns 0 since nothing is stored
    assert_eq!(buf.highest_seq(), 0);
}

#[test]
fn ack_beyond_delivered_rejected_under_backpressure() {
    let mut buf = ReceiveBuffer::new(100, 86400);
    for _ in 0..5 {
        buf.push(make_envelope(MessageKind::Notify));
    }

    // Fetch all 5 messages
    let (msgs, _, _) = buf.fetch("c1", 10, None);
    assert_eq!(msgs.len(), 5);

    // Only mark first 3 as actually delivered
    buf.update_delivered_seq("c1", 3);

    // Try to ack 4 — should fail because only 3 were delivered
    let result = buf.ack("c1", 4);
    assert_eq!(result, Err(AckError::OutOfRange));

    // Ack 3 should succeed
    let result = buf.ack("c1", 3);
    assert!(result.is_ok());
}

#[test]
fn consumer_state_is_bounded() {
    let mut buf = ReceiveBuffer::new(100, 86400).with_max_consumers(3);
    buf.push(make_envelope(MessageKind::Notify));

    for i in 0..5 {
        buf.fetch(&format!("consumer_{}", i), 10, None);
    }

    assert!(buf.consumers.len() <= 3);
}

#[test]
fn consumer_gc_resets_cursor() {
    let mut buf = ReceiveBuffer::new(100, 86400).with_max_consumers(2);
    for _ in 0..3 {
        buf.push(make_envelope(MessageKind::Notify));
    }

    // Consumer A fetches and acks
    let (msgs, _, _) = buf.fetch("a", 10, None);
    assert_eq!(msgs.len(), 3);
    buf.update_delivered_seq("a", 3);
    buf.ack("a", 2).unwrap();

    // Sleep to ensure consumer B and C get a later timestamp than A
    std::thread::sleep(std::time::Duration::from_millis(2));

    // Consumer B fetches (2 consumers <= max)
    buf.fetch("b", 10, None);

    // Consumer C should trigger GC, evicting the LRU (consumer A)
    buf.fetch("c", 10, None);
    assert!(buf.consumers.len() <= 2);
    assert!(
        !buf.consumers.contains_key("a"),
        "consumer A should be evicted as LRU"
    );

    // Re-create consumer A (evicted, so acked_seq resets to 0)
    let (msgs, _, _) = buf.fetch("a", 10, None);
    assert_eq!(
        msgs.len(),
        3,
        "evicted consumer should see all messages from acked_seq=0"
    );
}

proptest! {
    #[test]
    fn fetch_respects_limit(limit in 1usize..=1000) {
        let mut buf = ReceiveBuffer::new(100, 86400);
        for _ in 0..10 {
            buf.push(make_envelope(MessageKind::Notify));
        }
        let (msgs, _, _) = buf.fetch("c1", limit, None);
        prop_assert!(msgs.len() <= limit);
    }

    #[test]
    fn push_never_exceeds_capacity(capacity in 1usize..50, count in 1usize..100) {
        let mut buf = ReceiveBuffer::new(capacity, 86400);
        for _ in 0..count {
            buf.push(make_envelope(MessageKind::Notify));
        }
        let (msgs, _, _) = buf.fetch("c1", 1000, None);
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
        let (msgs, _, _) = buf.fetch("c1", 1000, None);
        for (i, msg) in msgs.iter().enumerate() {
            prop_assert_eq!(msg.envelope.from.to_string(), format!("ed25519.sender{}", i));
            prop_assert_eq!(msg.seq, (i + 1) as u64);
        }
    }
}
