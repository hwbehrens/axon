use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::tests::{directory, identity, observation};
use super::*;

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

// ---------------------------------------------------------------------------
// Round-seven review pin (DEC-022): a refresh of an EXISTING observation ID
// must never be rejected by the per-peer capacity bound. The stale entry is
// withdrawn before the capacity check, so an actively refreshing peer's
// liveness (`observed_at`) keeps advancing even at the 16-observation limit;
// otherwise its observations would expire out from under it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refresh_at_observation_capacity_still_updates_liveness() {
    let (_root, directory) = directory().await;
    let remote = identity(41);
    let agent_id = remote.agent_id().clone();
    directory.enroll(remote, Vec::new()).await.expect("enroll");

    for index in 0..MAX_OBSERVATIONS_PER_PEER {
        let outcome = directory
            .observe(observation(
                41,
                &format!("mdns:cap:{index}"),
                &format!("127.0.0.1:{}", 7800 + index),
            ))
            .await;
        assert_eq!(outcome, ObserveOutcome::EnrolledPeerRefreshed);
    }

    // The enrolled record is now exactly at capacity. Refreshing an
    // existing ID must still succeed — a new ID at capacity is rejected,
    // but a refresh replaces its own slot.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let outcome = directory
        .observe(observation(41, "mdns:cap:0", "127.0.0.1:7800"))
        .await;
    assert_eq!(
        outcome,
        ObserveOutcome::EnrolledPeerRefreshed,
        "capacity must reject only genuinely new observation IDs"
    );

    let state = directory.state.read().await;
    let peer = state.enrolled.get(&agent_id).expect("enrolled");
    assert_eq!(peer.observations.len(), MAX_OBSERVATIONS_PER_PEER);
    let refreshed = peer
        .observations
        .get(&ObservationId::new("mdns:cap:0").expect("id"))
        .expect("refreshed observation stays live");
    let other = peer
        .observations
        .get(&ObservationId::new("mdns:cap:1").expect("id"))
        .expect("sibling observation");
    assert!(
        refreshed.observed_at > other.observed_at,
        "the refresh must advance observed_at, keeping the peer alive"
    );
}

#[tokio::test]
async fn candidate_refresh_at_observation_capacity_still_updates_liveness() {
    let (_root, directory) = directory().await;

    for (index, expected) in std::iter::once(ObserveOutcome::CandidateAdded)
        .chain(std::iter::repeat(ObserveOutcome::CandidateRefreshed))
        .take(MAX_OBSERVATIONS_PER_PEER)
        .enumerate()
    {
        let outcome = directory
            .observe(observation(
                42,
                &format!("mdns:cap:{index}"),
                &format!("127.0.0.1:{}", 7900 + index),
            ))
            .await;
        assert_eq!(outcome, expected);
    }

    // A genuinely NEW observation ID at capacity is still rejected...
    assert_eq!(
        directory
            .observe(observation(42, "mdns:cap:new", "127.0.0.1:7999"))
            .await,
        ObserveOutcome::CapacityReached,
        "capacity must still bound genuinely new observation IDs"
    );

    // ...but a refresh of an EXISTING ID replaces its own slot.
    let outcome = directory
        .observe(observation(42, "mdns:cap:0", "127.0.0.1:7900"))
        .await;
    assert_eq!(
        outcome,
        ObserveOutcome::CandidateRefreshed,
        "candidate refresh at capacity must replace its own slot"
    );

    let state = directory.state.read().await;
    let candidate = state
        .candidates
        .get(identity(42).agent_id())
        .expect("candidate");
    assert_eq!(candidate.observations.len(), MAX_OBSERVATIONS_PER_PEER);
}
