use base64::{Engine as _, engine::general_purpose::STANDARD};
use tempfile::tempdir;

use super::tests::identity;
use super::*;

fn store_document(peers: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "version": 1, "peers": peers }))
        .expect("encode fixture")
}

#[test]
fn store_decode_rejects_oversized_enrolled_set() {
    let peers: Vec<serde_json::Value> = (0..=MAX_ENROLLED_PEERS)
        .map(|index| {
            let peer = identity(index as u8);
            serde_json::json!({
                "agent_id": peer.agent_id().as_str(),
                "pubkey": peer.public_key(),
                "locators": []
            })
        })
        .collect();

    assert!(
        PeerStore::decode(&store_document(serde_json::Value::Array(peers))).is_err(),
        "more than MAX_ENROLLED_PEERS records must fail validation"
    );
}

#[test]
fn store_decode_rejects_oversized_locator_set() {
    let remote = identity(1);
    let locators: Vec<String> = (0..=MAX_LOCATORS_PER_PEER)
        .map(|index| format!("svc-{index}.internal:{}", 7100 + index))
        .collect();
    let document = store_document(serde_json::json!([{
        "agent_id": remote.agent_id().as_str(),
        "pubkey": remote.public_key(),
        "locators": locators
    }]));

    assert!(
        PeerStore::decode(&document).is_err(),
        "more than MAX_LOCATORS_PER_PEER locators must fail validation"
    );
}

#[test]
fn store_decode_rejects_wrong_version() {
    let remote = identity(2);
    let document = serde_json::to_vec(&serde_json::json!({
        "version": 999,
        "peers": [{
            "agent_id": remote.agent_id().as_str(),
            "pubkey": remote.public_key(),
            "locators": []
        }]
    }))
    .expect("encode fixture");

    assert!(PeerStore::decode(&document).is_err());
}

#[test]
fn store_decode_never_panics_on_arbitrary_bytes() {
    for input in [
        &b""[..],
        b"{",
        b"null",
        b"[]",
        b"{\"version\":1}",
        b"{\"version\":1,\"peers\":{}}",
        b"{\"version\":1,\"peers\":[{}]}",
        b"{\"version\":1,\"peers\":[{\"agent_id\":\"ed25519.\",\"public_key\":\"AAA\",\"locators\":[\":\"]}]}",
    ] {
        assert!(PeerStore::decode(input).is_err());
    }
}

fn store_key(seed: u16) -> (AgentId, String) {
    let mut key_bytes = [0u8; 32];
    key_bytes[..2].copy_from_slice(&seed.to_be_bytes());
    let key = STANDARD.encode(key_bytes);
    let agent_id = AgentId::from_pubkey_base64(&key).expect("valid test key");
    (agent_id, key)
}

#[test]
fn store_decode_accepts_exactly_max_enrolled_peers() {
    let peers: Vec<serde_json::Value> = (0..MAX_ENROLLED_PEERS)
        .map(|index| {
            let (agent_id, key) = store_key(index as u16);
            serde_json::json!({
                "agent_id": agent_id.as_str(),
                "pubkey": key,
                "locators": []
            })
        })
        .collect();

    let decoded = PeerStore::decode(&store_document(serde_json::Value::Array(peers)))
        .expect("a store at exactly MAX_ENROLLED_PEERS is valid");
    assert_eq!(decoded.len(), MAX_ENROLLED_PEERS);
}

#[test]
fn store_decode_accepts_exactly_max_locators_per_peer() {
    let (agent_id, key) = store_key(0);
    let locators: Vec<String> = (0..MAX_LOCATORS_PER_PEER)
        .map(|index| format!("svc-{index}.internal:{}", 7100 + index))
        .collect();
    let document = store_document(serde_json::json!([{
        "agent_id": agent_id.as_str(),
        "pubkey": key,
        "locators": locators
    }]));

    let decoded =
        PeerStore::decode(&document).expect("a peer at exactly MAX_LOCATORS_PER_PEER is valid");
    assert_eq!(decoded[0].locators.len(), MAX_LOCATORS_PER_PEER);
}

#[tokio::test]
async fn unreadable_store_fails_closed_instead_of_loading_empty() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().expect("tempdir");
    let path = root.path().join("peers.json");
    std::fs::write(&path, b"{\"version\":1,\"peers\":[]}").expect("seed store");
    std::fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod");
    // Root (and some sandboxes) bypass permission bits; if the file is
    // still readable the property under test cannot be exercised here.
    if fs::read(&path).is_ok() {
        let _ = std::fs::set_permissions(&path, fs::Permissions::from_mode(0o644));
        return;
    }

    let result = PeerStore::new(path.clone()).load().await;
    let _ = std::fs::set_permissions(&path, fs::Permissions::from_mode(0o644));

    assert!(
        result.is_err(),
        "an unreadable peer store must fail closed, not load as empty"
    );
}
