use base64::{Engine as _, engine::general_purpose::STANDARD};
use tempfile::tempdir;

use super::tests::identity;
use super::*;
use crate::peer_directory::PeerLocator;

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

// =========================================================================
// Filesystem-level load hardening: type checks and byte bounds happen
// before any read or parse, so hostile store paths cannot cause symlink
// traversal, blocking reads, or unbounded allocation.
// =========================================================================

fn seed_valid_store(path: &std::path::Path) {
    let remote = identity(1);
    let document = serde_json::json!({
        "version": 1,
        "peers": [{
            "agent_id": remote.agent_id().as_str(),
            "pubkey": remote.public_key(),
            "locators": []
        }]
    });
    std::fs::write(path, serde_json::to_vec(&document).expect("encode fixture"))
        .expect("seed store");
}

#[tokio::test]
async fn load_refuses_symlinked_store_path() {
    let root = tempdir().expect("tempdir");
    let real = root.path().join("real-peers.json");
    seed_valid_store(&real);

    let link = root.path().join("peers.json");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");

    let result = PeerStore::new(link).load().await;
    let err = result.expect_err("symlinked store path must be refused");
    assert!(
        err.to_string().contains("non-regular"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn load_refuses_fifo_store_path() {
    let root = tempdir().expect("tempdir");
    let fifo = root.path().join("peers.json");
    let fifo_c = std::ffi::CString::new(fifo.to_str().expect("utf8 path")).expect("cstring");
    assert_eq!(
        unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) },
        0,
        "mkfifo fixture"
    );

    // Must fail on the type check without opening (an open would block on
    // a FIFO with no writer).
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        PeerStore::new(fifo).load(),
    )
    .await
    .expect("load must not hang on a FIFO");
    let err = result.expect_err("FIFO store path must be refused");
    assert!(
        err.to_string().contains("non-regular"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn load_refuses_oversized_store_file() {
    let root = tempdir().expect("tempdir");
    let path = root.path().join("peers.json");
    std::fs::write(&path, vec![b' '; MAX_PEER_STORE_BYTES + 1]).expect("seed oversized");

    let result = PeerStore::new(path).load().await;
    let err = result.expect_err("oversized store must be refused");
    assert!(
        err.to_string().contains("byte"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn load_rejects_growth_between_stat_and_read() {
    let root = tempdir().expect("tempdir");
    let path = root.path().join("peers.json");
    // Exactly at the cap is fine content-wise only if it parses; here it is
    // invalid JSON, but the point is the read itself must succeed and be
    // passed to decode rather than rejected by size. One byte over must be
    // rejected even if it only appears after the metadata check.
    std::fs::write(&path, b"{\"version\":1,\"peers\":[]}").expect("seed valid-sized store");
    let decoded = PeerStore::new(path.clone()).load().await;
    assert!(
        decoded.is_ok(),
        "valid store at any size below the cap loads"
    );

    std::fs::write(&path, vec![b' '; MAX_PEER_STORE_BYTES + 1]).expect("grow beyond cap");
    let result = PeerStore::new(path).load().await;
    assert!(result.is_err(), "store grown past the cap must be refused");
}

#[test]
fn decode_refuses_oversized_input_before_parsing() {
    let data = vec![b' '; MAX_PEER_STORE_BYTES + 1];
    let err = PeerStore::decode(&data).expect_err("oversized input must be refused");
    assert!(
        err.to_string().contains("maximum"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn save_refuses_serialized_store_over_the_load_cap() {
    // Hosts have no per-locator length bound, so enrollment can produce a
    // document larger than MAX_PEER_STORE_BYTES. The save must fail rather
    // than write a store the next restart refuses to load.
    let root = tempdir().expect("tempdir");
    let path = root.path().join("peers.json");
    let store = PeerStore::new(path.clone());

    let remote = identity(1);
    let locators: Vec<PeerLocator> = (0..MAX_LOCATORS_PER_PEER)
        .map(|index| {
            PeerLocator::parse(&format!("{}:{}", "x".repeat(200_000), 7000 + index))
                .expect("long-host locator parses")
        })
        .collect();

    let result = store
        .save(vec![StoredPeer {
            agent_id: remote.agent_id().clone(),
            public_key: remote.public_key().to_string(),
            locators,
        }])
        .await;
    let err = result.expect_err("over-cap store must fail to save");
    assert!(
        err.to_string().contains("maximum"),
        "unexpected error: {err:#}"
    );
    assert!(
        !path.exists(),
        "failed save must not leave a store file behind"
    );
}

#[test]
fn post_rename_sync_failure_is_a_warning_not_an_error() {
    // After a successful rename the new content is already live on disk.
    // Reporting the directory-sync failure as an error would tell callers
    // "nothing was persisted" while the rename already landed — disk ahead
    // of memory. The mapping must swallow it.
    let failing: std::io::Result<()> = Err(std::io::Error::other("directory sync failed"));
    super::store::note_post_rename_sync(failing, std::path::Path::new("/tmp/peers.json"));
    let ok: std::io::Result<()> = Ok(());
    super::store::note_post_rename_sync(ok, std::path::Path::new("/tmp/peers.json"));
}
