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

// ---------------------------------------------------------------------------
// Persistence without disk I/O under the state lock
//
// The persisting edits (enroll_candidate / enroll / remove_peer) save the
// peer store with no lock held and then apply their delta under a short
// commit lock. These tests pin the observable contract: persistence is
// correct across reload, revocation frees the identity for fresh discovery,
// and concurrent ephemeral observations are never clobbered by a persistent
// edit's commit.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn remove_peer_allows_reobservation_as_fresh_candidate() {
    let (_root, directory) = directory().await;
    let remote = identity(21);
    let agent_id = remote.agent_id().clone();

    directory
        .observe(observation(21, "mdns:revoke", "127.0.0.1:7102"))
        .await;
    let enrolled_identity = directory.enroll_candidate(&agent_id).await.expect("enroll");
    assert_eq!(
        directory.get_enrolled(&agent_id).await,
        Some(enrolled_identity.clone())
    );

    let removed = directory.remove_peer(&agent_id).await.expect("remove");
    assert_eq!(removed, enrolled_identity);
    assert_eq!(directory.get_enrolled(&agent_id).await, None);

    // Re-observation after revocation must start a FRESH candidate: not an
    // error, not a conflict against the revoked peer's stale claims.
    let outcome = directory
        .observe(observation(21, "mdns:revoke", "127.0.0.1:7102"))
        .await;
    assert_eq!(outcome, ObserveOutcome::CandidateAdded);
}

#[tokio::test]
async fn remove_peer_persists_across_reload() {
    let (root, directory) = directory().await;
    let keep = identity(22);
    let drop_peer = identity(23);
    let drop_agent_id = drop_peer.agent_id().clone();

    directory
        .enroll(
            keep.clone(),
            vec![PeerLocator::Socket("127.0.0.1:7103".parse().expect("addr"))],
        )
        .await
        .expect("enroll keep");
    directory
        .enroll(
            drop_peer,
            vec![PeerLocator::Socket("127.0.0.1:7104".parse().expect("addr"))],
        )
        .await
        .expect("enroll drop");

    directory.remove_peer(&drop_agent_id).await.expect("remove");

    let reloaded = PeerDirectory::load(
        identity(99).agent_id().clone(),
        PeerStore::new(root.path().join("peers.json")),
    )
    .await
    .expect("reload");
    assert!(reloaded.get_enrolled(&drop_agent_id).await.is_none());
    assert!(
        reloaded.get_enrolled(keep.agent_id()).await.is_some(),
        "unrelated enrolled peer must survive another peer's persisted removal"
    );
    assert_eq!(reloaded.list().await.len(), 1);
}

#[tokio::test]
async fn concurrent_observations_are_not_clobbered_by_persistent_edits() {
    use std::sync::Arc;

    let (_root, dir) = directory().await;
    let directory = Arc::new(dir);
    let observer = directory.clone();
    let editor = directory.clone();
    let churn_identity = identity(24);
    let churn_agent_id = churn_identity.agent_id().clone();

    // Ephemeral-only mutations run concurrently with a loop of persistent
    // edits. Under the old whole-snapshot swap this could resurrect or erase
    // candidate observations; with delta-apply commits the candidate must be
    // present afterwards.
    let churn = tokio::spawn(async move {
        for round in 0..64u32 {
            let id = ObservationId::new(format!("mdns:churn:{round}")).expect("id");
            let observation = PeerObservation::new(
                id,
                churn_identity.agent_id().clone(),
                churn_identity.public_key(),
                Some(format!("127.0.0.1:{}", 7200 + round).parse().expect("addr")),
                None,
                ObservationSource::Mdns,
            )
            .expect("observation");
            let _ = observer.observe(observation).await;
        }
    });

    let target = identity(25);
    for _ in 0..16 {
        let _ = editor
            .enroll(
                target.clone(),
                vec![PeerLocator::Socket("127.0.0.1:7300".parse().expect("addr"))],
            )
            .await
            .expect("enroll");
        editor.remove_peer(target.agent_id()).await.expect("remove");
    }
    churn.await.expect("churn task");

    let views = directory.list().await;
    let churn_view = views
        .iter()
        .find(|view| view.identity.agent_id() == &churn_agent_id)
        .expect("concurrently observed candidate must survive persistent-edit commits");
    assert_eq!(
        churn_view.observed_endpoints.len(),
        MAX_OBSERVATIONS_PER_PEER,
        "ephemeral observations must survive persistent-edit commits (bounded \
         only by the documented per-peer observation cap)"
    );
}
