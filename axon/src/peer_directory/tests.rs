use std::net::SocketAddr;
use std::time::Duration;

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
                "public_key": key,
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
        "public_key": key,
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

#[tokio::test]
async fn enrolled_views_expose_configured_locators_and_display_names() {
    let (_root, directory) = directory().await;
    let remote = identity(11);
    directory
        .observe(observation(11, "mdns:eleven", "127.0.0.1:7121"))
        .await;
    directory
        .enroll(
            remote.clone(),
            vec![PeerLocator::parse("peer.local:7100").expect("locator")],
        )
        .await
        .expect("enroll peer");

    let view = directory
        .list()
        .await
        .into_iter()
        .find(|view| view.identity.agent_id() == remote.agent_id())
        .expect("enrolled peer appears in views");

    assert_eq!(view.trust, PeerTrust::Enrolled);
    assert_eq!(
        view.configured_locators,
        vec![PeerLocator::parse("peer.local:7100").unwrap()],
        "views must surface configured locators"
    );
    assert!(
        !view.observed_endpoints.is_empty(),
        "live observations must appear as observed endpoints"
    );
}

#[tokio::test]
async fn display_name_reflects_the_most_recent_observation() {
    let (_root, directory) = directory().await;
    let remote = identity(12);

    let mut older = observation(12, "mdns:old", "127.0.0.1:7131");
    older.display_name = Some("old-name".into());
    let mut newer = observation(12, "mdns:new", "127.0.0.1:7132");
    newer.display_name = Some("new-name".into());
    newer.observed_at += Duration::from_secs(30);

    directory.observe(older).await;
    directory.observe(newer).await;

    let view = directory
        .list()
        .await
        .into_iter()
        .find(|view| view.identity.agent_id() == remote.agent_id())
        .expect("candidate appears in views");

    assert_eq!(
        view.display_name.as_deref(),
        Some("new-name"),
        "display name must come from the most recent observation"
    );
}

#[tokio::test]
async fn host_locator_resolution_prefers_ipv4_and_dedupes() {
    let locator = PeerLocator::parse("localhost:7100").expect("valid locator");

    let resolved = locator.resolve().await.expect("localhost resolves");

    assert!(!resolved.is_empty());
    assert!(
        resolved.iter().all(|addr| addr.port() == 7100),
        "resolution must preserve the locator port"
    );
    assert!(
        resolved
            .windows(2)
            .all(|pair| !(pair[0].is_ipv6() && pair[1].is_ipv4())),
        "IPv4 addresses must sort ahead of IPv6, got {resolved:?}"
    );
}

#[tokio::test]
async fn enroll_rejects_the_peer_beyond_max_enrolled_peers() {
    let (_root, directory) = directory().await;
    for index in 0..MAX_ENROLLED_PEERS {
        let mut key_bytes = [0u8; 32];
        key_bytes[..2].copy_from_slice(&(index as u16).to_be_bytes());
        let peer =
            PeerIdentity::from_public_key(&STANDARD.encode(key_bytes)).expect("valid test key");
        directory
            .enroll(peer, Vec::new())
            .await
            .unwrap_or_else(|err| panic!("enroll {index} failed: {err}"));
    }
    assert_eq!(
        directory.enrolled_agent_ids().await.len(),
        MAX_ENROLLED_PEERS
    );

    // Exactly at the bound the next identity is rejected.
    let overflow =
        PeerIdentity::from_public_key(&STANDARD.encode([255u8; 32])).expect("valid test key");
    assert!(
        directory.enroll(overflow, Vec::new()).await.is_err(),
        "enrollment beyond MAX_ENROLLED_PEERS must be rejected"
    );
}

#[tokio::test]
async fn enroll_rejects_locators_beyond_max_per_peer() {
    let (_root, directory) = directory().await;
    let remote = identity(20);
    let locators: Vec<PeerLocator> = (0..=MAX_LOCATORS_PER_PEER)
        .map(|index| PeerLocator::parse(&format!("svc-{index}.internal:{}", 7100 + index)).unwrap())
        .collect();

    assert!(
        directory.enroll(remote, locators).await.is_err(),
        "enrollment beyond MAX_LOCATORS_PER_PEER must be rejected"
    );
    assert!(
        directory
            .get_enrolled(identity(20).agent_id())
            .await
            .is_none(),
        "a rejected enrollment must not partially enroll the peer"
    );
}

#[test]
fn locator_parse_rejects_empty_hosts() {
    assert!(
        PeerLocator::parse(":7100").is_err(),
        "empty host must be rejected"
    );
    assert!(
        PeerLocator::parse("host with space:7100").is_err(),
        "whitespace in host must be rejected"
    );
}

#[test]
fn observation_id_display_preserves_the_value() {
    let id = ObservationId::new("mdns:instance:1").expect("valid observation id");
    assert_eq!(format!("{id}"), "mdns:instance:1");
}

#[tokio::test]
async fn expire_observations_removes_only_stale_claims() {
    let (_root, directory) = directory().await;
    let _fresh = identity(21);
    let stale = identity(22);
    let mut stale_observation = observation(22, "mdns:stale", "127.0.0.1:7141");
    stale_observation.observed_at -= OBSERVATION_STALE_TIMEOUT + Duration::from_secs(1);

    directory
        .observe(observation(21, "mdns:fresh", "127.0.0.1:7142"))
        .await;
    directory.observe(stale_observation).await;

    let removed = directory
        .expire_observations(std::time::Instant::now(), OBSERVATION_STALE_TIMEOUT)
        .await;
    assert_eq!(removed, vec![stale.agent_id().clone()]);
    assert_eq!(
        directory
            .observe(observation(22, "mdns:stale", "127.0.0.1:7143"))
            .await,
        ObserveOutcome::CandidateAdded,
        "expired candidates must be fully forgotten"
    );
}

#[tokio::test]
async fn observe_rejects_new_candidates_beyond_max_candidate_peers() {
    let (_root, directory) = directory().await;
    for index in 0..MAX_CANDIDATE_PEERS {
        let mut key_bytes = [0u8; 32];
        key_bytes[..2].copy_from_slice(&(index as u16).to_be_bytes());
        let key = STANDARD.encode(key_bytes);
        let agent_id = AgentId::from_pubkey_base64(&key).expect("valid test key");
        let candidate = PeerObservation::new(
            ObservationId::new(format!("mdns:fill:{index}")).expect("valid observation id"),
            agent_id,
            &key,
            None,
            None,
            ObservationSource::Mdns,
        )
        .expect("valid observation");
        assert_eq!(
            directory.observe(candidate).await,
            ObserveOutcome::CandidateAdded,
            "candidate {index} must be admitted below the bound"
        );
    }

    // A brand-new identity beyond the bound is rejected without evicting
    // any existing candidate.
    assert_eq!(
        directory
            .observe(observation(30, "mdns:overflow", "127.0.0.1:7151"))
            .await,
        ObserveOutcome::CapacityReached
    );
    // Refreshing an existing candidate still works at capacity.
    assert_eq!(
        directory
            .observe(observation(0, "mdns:fill:0", "127.0.0.1:7152"))
            .await,
        ObserveOutcome::CandidateRefreshed
    );
}

#[tokio::test]
async fn observations_exactly_at_ttl_are_not_expired() {
    let (_root, directory) = directory().await;
    let mut boundary = observation(31, "mdns:boundary", "127.0.0.1:7161");
    boundary.observed_at -= OBSERVATION_STALE_TIMEOUT;

    let boundary_time = boundary.observed_at;
    directory.observe(boundary).await;
    // Injected clock: age is exactly TTL, exercising the strict > boundary.
    let now = boundary_time + OBSERVATION_STALE_TIMEOUT;
    let removed = directory
        .expire_observations(now, OBSERVATION_STALE_TIMEOUT)
        .await;

    assert!(
        removed.is_empty(),
        "expiry is strictly greater-than: an observation exactly at TTL survives"
    );
    let removed = directory
        .expire_observations(now + Duration::from_nanos(1), OBSERVATION_STALE_TIMEOUT)
        .await;
    assert_eq!(
        removed.len(),
        1,
        "one nanosecond past TTL the observation expires"
    );
}

#[tokio::test]
async fn reenrolling_an_existing_peer_at_capacity_updates_locators() {
    let (_root, directory) = directory().await;
    for index in 0..MAX_ENROLLED_PEERS {
        let mut key_bytes = [0u8; 32];
        key_bytes[..2].copy_from_slice(&(index as u16).to_be_bytes());
        let peer =
            PeerIdentity::from_public_key(&STANDARD.encode(key_bytes)).expect("valid test key");
        directory
            .enroll(peer, Vec::new())
            .await
            .expect("fill peers");
    }

    // Re-enrolling an already-trusted peer is an update, not a new trust
    // grant: it must succeed even though the enrolled set is at capacity.
    let mut key_bytes = [0u8; 32];
    key_bytes[..2].copy_from_slice(&0u16.to_be_bytes());
    let existing =
        PeerIdentity::from_public_key(&STANDARD.encode(key_bytes)).expect("valid test key");
    let updated = directory
        .enroll(
            existing,
            vec![PeerLocator::parse("peer.local:7100").expect("locator")],
        )
        .await
        .expect("re-enrolling an enrolled peer at capacity is an update");

    let view = directory
        .list()
        .await
        .into_iter()
        .find(|view| view.identity.agent_id() == updated.agent_id())
        .expect("peer still enrolled");
    assert_eq!(view.configured_locators.len(), 1);
}

#[tokio::test]
async fn enroll_accepts_exactly_max_locators_then_rejects_more() {
    let (_root, directory) = directory().await;
    let remote = identity(32);
    let locators: Vec<PeerLocator> = (0..MAX_LOCATORS_PER_PEER)
        .map(|index| PeerLocator::parse(&format!("svc-{index}.internal:{}", 7100 + index)).unwrap())
        .collect();

    directory
        .enroll(remote.clone(), locators)
        .await
        .expect("a locator set of exactly MAX_LOCATORS_PER_PEER is valid");

    // Accumulating one more across calls trips the same bound.
    let overflow = PeerLocator::parse("svc-overflow.internal:7199").unwrap();
    assert!(
        directory.enroll(remote, vec![overflow]).await.is_err(),
        "accumulated locators beyond MAX_LOCATORS_PER_PEER must be rejected"
    );
}
