//! Fuzz target: deserialize arbitrary JSON as a capability `Manifest`.
//! Exercises both the `serve` publication path and the `describe` response
//! path. Must not panic regardless of input; invalid manifests must be
//! rejected as errors, never half-validated.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::manifest::{Manifest, MAX_MANIFEST_BYTES};

fuzz_target!(|data: &[u8]| {
    let Ok(manifest) = serde_json::from_slice::<Manifest>(data) else {
        return;
    };
    // A manifest that parses must satisfy every invariant the daemon relies
    // on: bounded service count, encoded size within the wire budget, and
    // successful re-encoding (the describe response path).
    assert!(!manifest.services.is_empty());
    assert!(manifest.services.len() <= 64);
    let size = manifest
        .encoded_size()
        .expect("a parsed manifest always re-encodes");
    assert!(size <= MAX_MANIFEST_BYTES);
});
