//! Fuzz target: deserialize arbitrary input as a `Vec<KnownPeer>` from JSON.
//! Uses lossy UTF-8 conversion since the known peers file is text-based.
//! Must not panic regardless of input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::config::KnownPeer;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = serde_json::from_str::<Vec<KnownPeer>>(&text);
});
