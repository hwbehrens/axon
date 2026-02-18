//! Fuzz target: deserialize arbitrary input as AXON persisted config YAML.
//! Uses lossy UTF-8 conversion since config files are text-based.
//! Must not panic regardless of input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::config::PersistedConfig;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = serde_yaml::from_str::<PersistedConfig>(&text);
});
