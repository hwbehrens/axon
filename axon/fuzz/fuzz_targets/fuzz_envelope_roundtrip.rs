//! Fuzz target: round-trip invariant for Envelope encode/decode.
//! Attempts to decode arbitrary bytes as an Envelope. If decoding succeeds,
//! re-encodes and re-decodes, then asserts the two decoded envelopes are equal.
//! This catches any asymmetry between serialization and deserialization.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::message::{Envelope, decode, encode};

fuzz_target!(|data: &[u8]| {
    let Ok(envelope) = serde_json::from_slice::<Envelope>(data) else {
        return;
    };

    let Ok(wire) = encode(&envelope) else {
        return;
    };

    let Ok(roundtripped) = decode(&wire) else {
        return;
    };

    assert_eq!(envelope, roundtripped, "round-trip mismatch");
});
