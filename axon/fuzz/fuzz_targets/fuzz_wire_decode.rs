//! Fuzz target: feed arbitrary bytes to `axon::message::decode()` (wire format).
//! Also exercises the length-prefix path: if input >= 4 bytes, extract the u32 BE
//! length prefix and verify that both the raw and stripped paths are handled safely.
//! Must not panic regardless of input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::message::{MAX_MESSAGE_SIZE, decode};

fuzz_target!(|data: &[u8]| {
    // Direct decode (no length prefix — this is what decode() expects).
    let _ = decode(data);

    // Exercise the length-prefix path manually.
    if data.len() >= 4 {
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let body = &data[4..];
        if len <= MAX_MESSAGE_SIZE && body.len() >= len as usize {
            let _ = decode(&body[..len as usize]);
        }
    }
});
