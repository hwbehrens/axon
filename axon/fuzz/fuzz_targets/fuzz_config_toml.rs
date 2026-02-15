//! Fuzz target: deserialize arbitrary input as an AXON `Config` from TOML.
//! Uses lossy UTF-8 conversion since config files are text-based.
//! Must not panic regardless of input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::config::Config;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = toml::from_str::<Config>(&text);
});
