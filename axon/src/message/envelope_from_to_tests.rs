//! D5 regression coverage: daemon-local `from`/`to` are ignore-unknown.
//!
//! `from`/`to` are not on the wire (`spec/WIRE_FORMAT.md` §6.2). A frame that
//! carries them must be decoded like any unknown field: a malformed value must
//! never drop the envelope, and a well-formed value must never be trusted as
//! identity (the transport overwrites both after decode).

use super::*;
use proptest::prelude::*;
use serde_json::json;

#[test]
fn malformed_from_to_values_never_drop_an_envelope() {
    for (from, to) in [
        (json!(42), json!(true)),
        (
            json!(["ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4"]),
            json!({}),
        ),
        (json!("not-an-agent-id"), json!("ed25519.zzzz")),
        (json!(null), json!(null)),
    ] {
        let frame = json!({
            "id": "6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
            "kind": "message",
            "payload": {"ok": true},
            "from": from,
            "to": to,
        });

        let decoded: Envelope =
            serde_json::from_value(frame).expect("malformed from/to must not fail decode");

        assert_eq!(
            decoded.from, None,
            "a malformed from is ignored, never kept"
        );
        assert_eq!(decoded.to, None, "a malformed to is ignored, never kept");
        assert_eq!(decoded.kind, MessageKind::Message);
        assert_eq!(decoded.payload_value().unwrap()["ok"], json!(true));
        assert!(
            decoded.validate().is_ok(),
            "the rest of the frame stays valid"
        );
    }
}

#[test]
fn malformed_from_to_values_drop_from_the_round_tripped_form() {
    let raw = br#"{
            "id":"6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
            "kind":"message",
            "from":"not-an-agent-id",
            "to":7,
            "payload":{"ok":true}
        }"#;

    let decoded = decode(raw).expect("wire decode must tolerate malformed from/to");
    assert_eq!(decoded.from, None);
    assert_eq!(decoded.to, None);

    // Re-encoding omits the malformed values entirely, so the frame
    // round-trips as a clean wire message.
    let reencoded = encode(&decoded).expect("re-encode");
    let round_tripped = decode(&reencoded).expect("re-decode");
    assert_eq!(round_tripped, decoded);
    let json: serde_json::Value = serde_json::from_slice(&reencoded).unwrap();
    assert!(json.get("from").is_none());
    assert!(json.get("to").is_none());
}

#[test]
fn wire_frames_tolerate_object_array_and_null_from_to() {
    // D5 wire-path coverage: unlike `serde_json::from_value`, the streaming
    // `decode` path leaves an unconsumed `{`/`[` token when a `String`
    // deserializer rejects it, so object/array `from`/`to` used to drop the
    // whole frame instead of being ignored like any other unknown field.
    for shape in [json!({}), json!(["nested", 1]), json!(null)] {
        let frame = json!({
            "id": "6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
            "kind": "message",
            "payload": {"ok": true},
            "from": shape.clone(),
            "to": shape,
        });
        let raw = serde_json::to_vec(&frame).expect("frame serializes");
        let decoded = decode(&raw)
            .unwrap_or_else(|err| panic!("wire decode must tolerate from/to {frame}: {err}"));
        assert_eq!(decoded.from, None, "from {frame} must decode as None");
        assert_eq!(decoded.to, None, "to {frame} must decode as None");
        assert_eq!(decoded.kind, MessageKind::Message);
        assert_eq!(decoded.payload_value().unwrap()["ok"], json!(true));
    }
}

#[test]
fn well_formed_from_to_still_round_trip() {
    // IPC replies carry daemon-populated from/to; lenient decoding must not
    // discard well-formed values.
    let envelope = Envelope::new(agent_a(), agent_b(), MessageKind::Request, json!({}));
    let decoded = decode(&encode(&envelope).unwrap()).expect("decode");
    assert_eq!(decoded.from.as_deref(), Some(agent_a().as_str()));
    assert_eq!(decoded.to.as_deref(), Some(agent_b().as_str()));
}

proptest! {
    #[test]
    fn arbitrary_from_to_strings_never_panic_or_fail_decode(raw in "\\PC{0,80}") {
        let frame = json!({
            "id": "6fc0ec4f-e59f-4bea-9d57-0d9fdd1108f1",
            "kind": "message",
            "payload": {},
            "from": raw.clone(),
            "to": raw,
        });
        let decoded: Envelope = serde_json::from_value(frame).expect("decode must not fail");
        // Only a canonically valid Agent ID may survive; anything else is None.
        prop_assert!(
            decoded.from.is_none()
                || AgentId::parse(decoded.from.as_ref().unwrap().as_str()).is_ok()
        );
    }
}

fn agent_a() -> AgentId {
    AgentId::parse("ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4").unwrap()
}

fn agent_b() -> AgentId {
    AgentId::parse("ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3").unwrap()
}
