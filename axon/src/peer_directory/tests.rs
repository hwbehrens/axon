use std::net::SocketAddr;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tempfile::tempdir;

use super::*;

fn identity(seed: u8) -> PeerIdentity {
    PeerIdentity::from_public_key(&STANDARD.encode([seed; 32])).expect("valid test key")
}

fn observation(seed: u8, id: &str, endpoint: &str) -> PeerObservation {
    let identity = identity(seed);
    PeerObservation::new(
        ObservationId::new(id).expect("valid observation id"),
        identity.agent_id().clone(),
        identity.public_key(),
        Some(endpoint.parse::<SocketAddr>().expect("valid endpoint")),
        None,
        ObservationSource::Mdns,
    )
    .expect("valid observation")
}

async fn directory() -> (tempfile::TempDir, PeerDirectory) {
    let root = tempdir().expect("tempdir");
    let local = identity(99).agent_id().clone();
    let store = PeerStore::new(root.path().join("peers.json"));
    let directory = PeerDirectory::load(local, store)
        .await
        .expect("load empty directory");
    (root, directory)
}

#[tokio::test]
async fn discovery_candidate_does_not_enter_tls_pin_snapshot() {
    let (_root, directory) = directory().await;
    let remote = identity(1);

    let outcome = directory
        .observe(observation(1, "mdns:one", "127.0.0.1:7100"))
        .await;

    assert_eq!(outcome, ObserveOutcome::CandidateAdded);
    let pins = directory.pinning_snapshot();
    assert!(
        !pins
            .read()
            .expect("pin snapshot lock")
            .contains_key(remote.agent_id().as_str()),
        "an untrusted discovery observation must not establish TLS trust"
    );
}

#[tokio::test]
async fn explicit_candidate_enrollment_persists_before_publishing_pin() {
    let (root, directory) = directory().await;
    let remote = identity(2);
    directory
        .observe(observation(2, "mdns:two", "127.0.0.1:7101"))
        .await;

    directory
        .enroll_candidate(remote.agent_id())
        .await
        .expect("enroll candidate");

    assert_eq!(
        directory
            .pinning_snapshot()
            .read()
            .expect("pin snapshot lock")
            .get(remote.agent_id().as_str()),
        Some(&remote.public_key().to_string())
    );
    let reloaded = PeerDirectory::load(
        identity(99).agent_id().clone(),
        PeerStore::new(root.path().join("peers.json")),
    )
    .await
    .expect("reload persisted directory");
    assert!(reloaded.get_enrolled(remote.agent_id()).await.is_some());
}

#[tokio::test]
async fn removing_peer_removes_durable_and_tls_authority() {
    let (root, directory) = directory().await;
    let remote = identity(3);
    directory
        .enroll(
            remote.clone(),
            vec![PeerLocator::parse("peer.local:7100").expect("locator")],
        )
        .await
        .expect("enroll peer");

    directory
        .remove_peer(remote.agent_id())
        .await
        .expect("remove peer");

    assert!(directory.get_enrolled(remote.agent_id()).await.is_none());
    assert!(
        !directory
            .pinning_snapshot()
            .read()
            .expect("pin snapshot lock")
            .contains_key(remote.agent_id().as_str())
    );
    let reloaded = PeerDirectory::load(
        identity(99).agent_id().clone(),
        PeerStore::new(root.path().join("peers.json")),
    )
    .await
    .expect("reload persisted directory");
    assert!(reloaded.get_enrolled(remote.agent_id()).await.is_none());
}

#[tokio::test]
async fn conflicting_endpoint_is_not_returned_as_a_dial_target() {
    let (_root, directory) = directory().await;
    let first = identity(4);
    let second = identity(5);
    directory
        .observe(observation(4, "mdns:first", "127.0.0.1:7102"))
        .await;
    directory
        .enroll_candidate(first.agent_id())
        .await
        .expect("enroll first");

    let outcome = directory
        .observe(observation(5, "mdns:second", "127.0.0.1:7102"))
        .await;

    assert_eq!(outcome, ObserveOutcome::LocatorConflict);
    assert!(directory.dial_targets(first.agent_id()).await.is_empty());
    assert!(directory.dial_targets(second.agent_id()).await.is_empty());
}

#[tokio::test]
async fn malformed_store_fails_closed_without_partial_load() {
    let root = tempdir().expect("tempdir");
    let valid = identity(6);
    let wrong = identity(7);
    let document = serde_json::json!({
        "version": 1,
        "peers": [
            {
                "agent_id": valid.agent_id(),
                "public_key": wrong.public_key(),
                "locators": []
            }
        ]
    });
    std::fs::write(
        root.path().join("peers.json"),
        serde_json::to_vec(&document).expect("serialize fixture"),
    )
    .expect("write fixture");

    let result = PeerDirectory::load(
        identity(99).agent_id().clone(),
        PeerStore::new(root.path().join("peers.json")),
    )
    .await;

    assert!(
        result.is_err(),
        "mismatched identity must fail the whole load"
    );
    assert!(
        PeerStore::new(root.path().join("peers.json"))
            .validate()
            .await
            .is_err(),
        "the standalone store validator must enforce identity binding"
    );
}

#[tokio::test]
async fn duplicate_store_locators_fail_closed() {
    let root = tempdir().expect("tempdir");
    let remote = identity(8);
    let document = serde_json::json!({
        "version": 1,
        "peers": [{
            "agent_id": remote.agent_id(),
            "public_key": remote.public_key(),
            "locators": ["peer.local:7100", "peer.local:7100"]
        }]
    });
    std::fs::write(
        root.path().join("peers.json"),
        serde_json::to_vec(&document).expect("serialize fixture"),
    )
    .expect("write fixture");

    assert!(
        PeerStore::new(root.path().join("peers.json"))
            .validate()
            .await
            .is_err()
    );
}

#[tokio::test]
async fn revocation_removes_observation_index_entries() {
    let (_root, directory) = directory().await;
    let remote = identity(9);
    directory
        .observe(observation(9, "mdns:nine", "127.0.0.1:7109"))
        .await;
    directory
        .enroll_candidate(remote.agent_id())
        .await
        .expect("enroll candidate");
    directory
        .remove_peer(remote.agent_id())
        .await
        .expect("revoke peer");

    assert_eq!(
        directory
            .observe(observation(10, "mdns:nine", "127.0.0.1:7110"))
            .await,
        ObserveOutcome::CandidateAdded,
        "revocation must not leave a stale observation owner"
    );
}

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
                "public_key": peer.public_key(),
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
        "public_key": remote.public_key(),
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
            "public_key": remote.public_key(),
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
