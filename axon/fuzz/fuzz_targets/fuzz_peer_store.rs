//! Fuzz target: feed arbitrary bytes to `PeerStore::decode` (peers.json).
//! The peer store gates the TLS pin set, so parsing untrusted content must
//! never panic: every input either validates or returns an error.

#![no_main]

use libfuzzer_sys::fuzz_target;

use axon::peer_directory::{MAX_ENROLLED_PEERS, PeerStore};

fuzz_target!(|data: &[u8]| {
    if let Ok(peers) = PeerStore::decode(data) {
        // Any input that decodes must respect the same bounds the store
        // enforces on writes; a decoded set exceeding them would mean the
        // validator and the writer disagree about durable state.
        assert!(peers.len() <= MAX_ENROLLED_PEERS);
        // Decoding is pure: re-decoding the canonical re-encoding must agree.
        if let Ok(document) = serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "peers": peers,
        })) {
            let redecoded = PeerStore::decode(&document).expect("re-encoded store decodes");
            assert_eq!(redecoded, peers);
        }
    }
});
