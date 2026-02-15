//! Fuzz target: deserialize arbitrary input as an `IpcCommand` (tagged JSON enum).
//! Uses lossy UTF-8 conversion since IPC commands are text-based.
//! Must not panic regardless of input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::ipc::IpcCommand;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = serde_json::from_str::<IpcCommand>(&text);
});
