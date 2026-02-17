//! Fuzz target: feed arbitrary bytes to `axon::message::decode()` (wire format).
//! Must not panic regardless of input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::message::decode;

fuzz_target!(|data: &[u8]| {
    // decode() expects raw JSON bytes delimited by QUIC stream FIN.
    let _ = decode(data);
});
