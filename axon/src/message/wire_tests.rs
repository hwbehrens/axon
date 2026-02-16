use super::super::envelope::Envelope;
use super::super::kind::MessageKind;
use super::*;
use serde_json::json;

fn agent_a() -> String {
    "ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string()
}

fn agent_b() -> String {
    "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3".to_string()
}

#[test]
fn encode_decode_roundtrip() {
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Request,
        json!({"question": "test?"}),
    );
    let encoded = encode(&env).unwrap();
    let decoded = decode(&encoded).unwrap();
    assert_eq!(env, decoded);
}

#[test]
fn reject_oversized_message() {
    let big = "x".repeat(MAX_MESSAGE_SIZE as usize);
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Request,
        json!({"question": big}),
    );
    let result = encode(&env);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
}

#[test]
fn decode_rejects_oversized_input() {
    let data = vec![b'{'; MAX_MESSAGE_SIZE as usize + 1];
    let result = decode(&data);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
}

#[test]
fn now_millis_is_plausible() {
    let ms = now_millis();
    assert!(ms > 1_577_836_800_000);
}

// =========================================================================
// Property-based tests
// =========================================================================

use proptest::prelude::*;

const ALL_KINDS: &[MessageKind] = &[
    MessageKind::Request,
    MessageKind::Response,
    MessageKind::Message,
    MessageKind::Error,
];

proptest! {
    #[test]
    fn encode_decode_roundtrip_prop(payload_str in ".{0,1000}",
                                    kind_idx in 0..ALL_KINDS.len()) {
        let kind = ALL_KINDS[kind_idx];
        let env = Envelope::new(
            agent_a(),
            agent_b(),
            kind,
            json!({"data": payload_str}),
        );
        if let Ok(encoded) = encode(&env) {
            let decoded = decode(&encoded).unwrap();
            prop_assert_eq!(env, decoded);
        }
    }

    #[test]
    fn encoded_is_raw_json(payload_str in ".{0,500}") {
        let env = Envelope::new(
            agent_a(),
            agent_b(),
            MessageKind::Request,
            json!({"data": payload_str}),
        );
        if let Ok(encoded) = encode(&env) {
            let expected = serde_json::to_vec(&env).unwrap();
            prop_assert_eq!(encoded, expected);
        }
    }

    #[test]
    fn decode_arbitrary_bytes_never_panics(data in proptest::collection::vec(any::<u8>(), 0..128)) {
        let _ = decode(&data);
    }
}

// =========================================================================
// Mutation-coverage: encode accepts exactly MAX_MESSAGE_SIZE
// =========================================================================

#[test]
fn encode_accepts_exactly_max_size() {
    let env_template = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"data": ""}),
    );
    let base_len = serde_json::to_vec(&env_template).unwrap().len();
    assert!(base_len < MAX_MESSAGE_SIZE as usize);

    // Binary search for the padding length that makes JSON exactly MAX_MESSAGE_SIZE
    let target = MAX_MESSAGE_SIZE as usize;
    // The "data" field value is a string. Increasing by 1 char adds 1 byte to JSON
    // (unless the char needs escaping). Use 'a' which is safe.
    let needed = target - base_len;
    let padding = "a".repeat(needed);
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Message,
        json!({"data": padding}),
    );
    let json_len = serde_json::to_vec(&env).unwrap().len();
    assert_eq!(
        json_len, target,
        "JSON body should be exactly MAX_MESSAGE_SIZE"
    );
    assert!(
        encode(&env).is_ok(),
        "encode must accept exactly MAX_MESSAGE_SIZE"
    );
}
