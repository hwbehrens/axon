//! Fuzz target: feed arbitrary line sequences through IPC command deserialization.
//! Simulates a multi-command IPC session. Must not panic regardless of input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::ipc::IpcCommand;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let _ = serde_json::from_str::<IpcCommand>(line);
    }
});
