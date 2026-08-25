use std::net::SocketAddr;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tempfile::tempdir;

use super::*;

pub(super) fn identity(seed: u8) -> PeerIdentity {
    PeerIdentity::from_public_key(&STANDARD.encode([seed; 32])).expect("valid test key")
}

pub(super) fn observation(seed: u8, id: &str, endpoint: &str) -> PeerObservation {
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

pub(super) async fn directory() -> (tempfile::TempDir, PeerDirectory) {
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
                "pubkey": wrong.public_key(),
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
            "pubkey": remote.public_key(),
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

#[tokio::test]
async fn store_accepts_spec_pubkey_field_and_legacy_alias() {
    let root = tempdir().expect("tempdir");
    let remote = identity(40);
    // Canonical spec spelling (spec/SPEC.md "Peer Store Format").
    let canonical = serde_json::json!({
        "version": 1,
        "peers": [{
            "agent_id": remote.agent_id().as_str(),
            "pubkey": remote.public_key(),
            "locators": []
        }]
    });
    std::fs::write(
        root.path().join("canonical.json"),
        serde_json::to_vec(&canonical).expect("encode"),
    )
    .expect("write canonical");
    let canonical_store = PeerStore::new(root.path().join("canonical.json"));
    assert_eq!(canonical_store.validate().await.expect("spec form"), 1);

    // The pre-release field name keeps loading so early files do not strand.
    let legacy = serde_json::json!({
        "version": 1,
        "peers": [{
            "agent_id": remote.agent_id().as_str(),
            "public_key": remote.public_key(),
            "locators": []
        }]
    });
    std::fs::write(
        root.path().join("legacy.json"),
        serde_json::to_vec(&legacy).expect("encode"),
    )
    .expect("write legacy");
    let legacy_store = PeerStore::new(root.path().join("legacy.json"));
    assert_eq!(legacy_store.validate().await.expect("alias form"), 1);
}

#[tokio::test]
async fn revocation_unquarantines_survivors_of_conflicting_endpoints() {
    let (_root, directory) = directory().await;
    let first = identity(41);
    let second = identity(42);
    directory
        .observe(observation(41, "mdns:first", "127.0.0.1:7171"))
        .await;
    directory
        .enroll_candidate(first.agent_id())
        .await
        .expect("enroll first");
    // Second peer claims the same endpoint: both sides quarantine.
    assert_eq!(
        directory
            .observe(observation(42, "mdns:second", "127.0.0.1:7171"))
            .await,
        ObserveOutcome::LocatorConflict
    );
    assert!(directory.dial_targets(first.agent_id()).await.is_empty());

    // Enroll the conflicting peer so both sides hold trusted claims.
    directory.enroll_candidate(second.agent_id()).await.unwrap();

    // Revoking the conflicting claim must release the survivor's endpoint.
    directory.remove_peer(second.agent_id()).await.unwrap();
    assert!(
        !directory.dial_targets(first.agent_id()).await.is_empty(),
        "surviving peer must become dialable once the conflict is gone"
    );
}
