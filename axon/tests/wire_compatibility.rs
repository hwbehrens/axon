use axon::message::{Envelope, HelloPayload};
use serde_json::json;

#[test]
fn test_wire_envelope_serialization() {
    let env = Envelope::new(
        "sender".to_string(),
        "receiver".to_string(),
        "hello".to_string(),
        json!(HelloPayload {
            protocol_versions: vec![1],
            agent_name: None,
            features: vec![],
        })
    );
    
    let serialized = serde_json::to_string(&env).unwrap();
    let deserialized: Envelope = serde_json::from_str(&serialized).unwrap();
    
    assert_eq!(deserialized.v, 1);
    assert_eq!(deserialized.kind, "hello");
}

#[test]
fn test_big_endian_framing() {
    let len: u32 = 1024;
    let bytes = len.to_be_bytes();
    assert_eq!(bytes, [0, 0, 4, 0]); // Big-endian verification
}
