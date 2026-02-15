//! Fuzz target: deserialize arbitrary bytes as an `Envelope` via serde_json.
//! Must not panic regardless of input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::message::Envelope;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<Envelope>(data);
});
