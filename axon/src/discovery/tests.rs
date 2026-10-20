use base64::{Engine as _, engine::general_purpose::STANDARD};
use mdns_sd::ServiceInfo;

use super::*;
use crate::peer_directory::OBSERVATION_STALE_TIMEOUT;

fn identity(seed: u8) -> (AgentId, String) {
    let public_key = STANDARD.encode([seed; 32]);
    let agent_id = AgentId::from_pubkey_base64(&public_key).expect("valid test key");
    (agent_id, public_key)
}

fn service(agent_id: &AgentId, public_key: &str) -> ServiceInfo {
    let properties = [("agent_id", agent_id.as_str()), ("pubkey", public_key)];
    ServiceInfo::new(
        SERVICE_TYPE,
        "remote-agent",
        "remote-agent.local.",
        "192.168.1.20",
        7100,
        &properties[..],
    )
    .expect("service info")
}

fn service_with_addresses(agent_id: &AgentId, public_key: &str, addresses: &str) -> ServiceInfo {
    let properties = [("agent_id", agent_id.as_str()), ("pubkey", public_key)];
    ServiceInfo::new(
        SERVICE_TYPE,
        "remote-agent",
        "remote-agent.local.",
        addresses,
        7100,
        &properties[..],
    )
    .expect("service info")
}

fn address_list(count: u8, subnet: u8) -> String {
    (1..=count)
        .map(|host| format!("192.168.{subnet}.{host}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn valid_service_becomes_untrusted_observation() {
    let (local, _) = identity(1);
    let (remote, public_key) = identity(2);

    let observations =
        parse_resolved_service(&local, &service(&remote, &public_key)).expect("valid observation");

    assert_eq!(observations.len(), 1);
    let observation = &observations[0];
    assert_eq!(observation.identity.agent_id(), &remote);
    assert_eq!(observation.identity.public_key(), public_key);
    assert_eq!(observation.endpoint.expect("endpoint").port(), 7100);
    assert_eq!(observation.source, ObservationSource::Mdns);
}

#[test]
fn self_advertisement_is_ignored() {
    let (local, public_key) = identity(3);

    let observations = parse_resolved_service(&local, &service(&local, &public_key))
        .expect("self observation parses");

    assert!(observations.is_empty());
}

#[test]
fn advertised_agent_id_must_match_public_key() {
    let (local, _) = identity(4);
    let (claimed, _) = identity(5);
    let (_, other_public_key) = identity(6);

    let result = parse_resolved_service(&local, &service(&claimed, &other_public_key));

    assert!(
        result.is_err(),
        "identity mismatch must be rejected at discovery"
    );
}

#[test]
fn service_without_public_key_is_not_a_candidate() {
    let (local, _) = identity(7);
    let (remote, _) = identity(8);
    let properties = [("agent_id", remote.as_str())];
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "remote-agent",
        "remote-agent.local.",
        "192.168.1.21",
        7100,
        &properties[..],
    )
    .expect("service info");

    let observations = parse_resolved_service(&local, &info).expect("missing key is ignored");

    assert!(observations.is_empty());
}

#[test]
fn observation_id_is_endpoint_scoped() {
    let (local, _) = identity(9);
    let (remote, public_key) = identity(10);
    let properties = [
        ("agent_id", remote.as_str()),
        ("pubkey", public_key.as_str()),
    ];
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "remote-agent",
        "remote-agent.local.",
        "192.168.1.22,192.168.1.23",
        7100,
        &properties[..],
    )
    .expect("service info");

    let observations = parse_resolved_service(&local, &info).expect("valid observations");

    assert_eq!(observations.len(), 2);
    assert_ne!(observations[0].id, observations[1].id);
}

#[test]
fn service_without_agent_id_is_not_a_candidate() {
    let (local, _) = identity(11);
    let (_, public_key) = identity(12);
    // A valid-looking pubkey with no agent_id property cannot identify a peer.
    let properties = [("pubkey", public_key.as_str())];
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "remote-agent",
        "remote-agent.local.",
        "192.168.1.24",
        7100,
        &properties[..],
    )
    .expect("service info");

    let observations = parse_resolved_service(&local, &info).expect("missing id is ignored");

    assert!(observations.is_empty());
}

// ---------------------------------------------------------------------------
// ServiceTracker — bounded tracking under repeated refreshes
//
// Regression coverage for the insertion-order queue: a service whose stored
// observation set is empty (self, malformed, or address-less) refreshes
// periodically, and every refresh must keep its original queue position
// instead of re-appending.
// ---------------------------------------------------------------------------

fn empty_resolution(tracker: &mut ServiceTracker, fullname: &str) {
    let events = tracker.observe_resolution(fullname.to_string(), Vec::new());
    assert!(events.is_empty(), "empty resolution emits no events");
}

#[test]
fn repeated_empty_refresh_does_not_grow_insertion_order() {
    let mut tracker = ServiceTracker::new();

    for _ in 0..1_000 {
        empty_resolution(&mut tracker, "self-or-malformed._axon._udp.local.");
    }

    assert_eq!(tracker.observations_by_service.len(), 1);
    assert_eq!(
        tracker.insertion_order.len(),
        1,
        "refreshes of an already-tracked name must not re-append to the order queue"
    );
}

#[test]
fn valid_service_refresh_keeps_single_queue_entry() {
    let (local, _) = identity(13);
    let (remote, public_key) = identity(14);
    let info = service(&remote, &public_key);

    let mut tracker = ServiceTracker::new();
    let first = parse_resolved_service(&local, &info).expect("valid observation");
    let second = parse_resolved_service(&local, &info).expect("valid observation");

    let events = tracker.observe_resolution("remote._axon._udp.local.".to_string(), first);
    assert_eq!(events.len(), 1);
    let events = tracker.observe_resolution("remote._axon._udp.local.".to_string(), second);
    assert_eq!(events.len(), 1, "refresh emits one Observed event");

    assert_eq!(tracker.insertion_order.len(), 1);
}

#[test]
fn unique_empty_services_stay_bounded_at_capacity() {
    let mut tracker = ServiceTracker::new();

    // More unique names than MAX_TRACKED_SERVICES: each new name past the
    // cap evicts the oldest live entry, so neither structure may exceed it.
    for index in 0..(MAX_TRACKED_SERVICES + 64) {
        empty_resolution(&mut tracker, &format!("churn-{index}._axon._udp.local."));
    }

    assert_eq!(tracker.observations_by_service.len(), MAX_TRACKED_SERVICES);
    assert!(
        tracker.insertion_order.len() <= MAX_TRACKED_SERVICES,
        "order queue must stay bounded by the tracked-service cap"
    );
}

#[test]
fn removal_compacts_stale_queue_entries() {
    let mut tracker = ServiceTracker::new();
    for index in 0..64 {
        empty_resolution(&mut tracker, &format!("gone-{index}._axon._udp.local."));
    }

    for index in 0..64 {
        let events = tracker.remove_service(&format!("gone-{index}._axon._udp.local."));
        assert!(events.is_empty());
    }

    assert!(tracker.observations_by_service.is_empty());
    assert!(
        tracker.insertion_order.len() < 16,
        "compaction must prune stale entries: {}",
        tracker.insertion_order.len()
    );
}

// ---------------------------------------------------------------------------
// I2 — liveness under unchanged mDNS records
//
// mdns-sd 0.11.5 emits ServiceResolved only when a record is new or changes,
// so a live peer that keeps re-announcing identical records would go stale at
// the directory's 60s timeout and never come back. These tests pin the
// refresh contract that keeps such peers alive, using an explicit clock
// argument as the fake-clock shape (no wall-clock sleeps).
// ---------------------------------------------------------------------------

#[test]
fn refresh_interval_precedes_the_observation_stale_timeout() {
    assert!(
        MDNS_REFRESH_INTERVAL < OBSERVATION_STALE_TIMEOUT,
        "liveness refresh ({MDNS_REFRESH_INTERVAL:?}) must fire before observations \
         expire ({OBSERVATION_STALE_TIMEOUT:?})"
    );
}

#[test]
fn unchanged_records_re_refresh_with_the_supplied_clock() {
    let (local, _) = identity(15);
    let (remote, public_key) = identity(16);
    let observations =
        parse_resolved_service(&local, &service(&remote, &public_key)).expect("valid observation");
    let expected_id = observations[0].id.clone();

    let mut tracker = ServiceTracker::new();
    tracker.observe_resolution("remote._axon._udp.local.".to_string(), observations);

    // No further ServiceResolved arrives for unchanged records; every refresh
    // tick must instead re-emit the cached observation stamped with the tick
    // time, or the directory's stale sweep expires the live peer.
    let base = Instant::now();
    for tick in 1..=3u32 {
        let now = base + Duration::from_secs(u64::from(tick) * 30);
        let events = tracker.refresh_events(now);
        match events.as_slice() {
            [DiscoveryEvent::Observed(observation)] => {
                assert_eq!(
                    observation.id, expected_id,
                    "same live observation re-emitted"
                );
                assert_eq!(
                    observation.observed_at, now,
                    "refresh must stamp the tick time"
                );
            }
            other => panic!("expected exactly one Observed event, got {other:?}"),
        }
    }
}

#[test]
fn refresh_after_removal_emits_nothing() {
    let (local, _) = identity(17);
    let (remote, public_key) = identity(18);
    let name = "remote._axon._udp.local.".to_string();
    let observations =
        parse_resolved_service(&local, &service(&remote, &public_key)).expect("valid observation");

    let mut tracker = ServiceTracker::new();
    tracker.observe_resolution(name.clone(), observations);
    tracker.remove_service(&name);

    assert!(
        tracker.refresh_events(Instant::now()).is_empty(),
        "a withdrawn service must not be kept alive by refresh ticks"
    );
}

#[test]
fn re_resolved_service_is_not_double_reported_by_refresh() {
    let (local, _) = identity(19);
    let (remote, public_key) = identity(20);
    let name = "remote._axon._udp.local.".to_string();

    let mut tracker = ServiceTracker::new();
    tracker.observe_resolution(
        name.clone(),
        parse_resolved_service(&local, &service(&remote, &public_key)).expect("first resolve"),
    );
    tracker.remove_service(&name);
    tracker.observe_resolution(
        name,
        parse_resolved_service(&local, &service(&remote, &public_key)).expect("second resolve"),
    );

    assert_eq!(
        tracker.refresh_events(Instant::now()).len(),
        1,
        "one live service must appear exactly once per refresh"
    );
}

// ---------------------------------------------------------------------------
// I12 — hostile mDNS floods
//
// AXON cannot bound mdns-sd's record cache, so it bounds everything it tracks
// and forwards: at most MAX_OBSERVATIONS_PER_PEER live observations per
// advertised Agent ID and only dialable endpoint families.
// ---------------------------------------------------------------------------

#[test]
fn single_service_endpoints_are_capped_at_the_directory_limit() {
    let (local, _) = identity(21);
    let (remote, public_key) = identity(22);
    let info = service_with_addresses(&remote, &public_key, &address_list(24, 3));

    let observations = parse_resolved_service(&local, &info).expect("service parses");

    assert_eq!(
        observations.len(),
        MAX_OBSERVATIONS_PER_PEER,
        "one service may not exceed the directory's per-peer observation bound"
    );
}

#[test]
fn tracked_observations_per_agent_stay_within_the_directory_limit() {
    let (local, _) = identity(23);
    let (remote, public_key) = identity(24);
    let first = service_with_addresses(&remote, &public_key, &address_list(10, 1));
    let second = service_with_addresses(&remote, &public_key, &address_list(10, 2));

    let mut tracker = ServiceTracker::new();
    let events = tracker.observe_resolution(
        "first._axon._udp.local.".to_string(),
        parse_resolved_service(&local, &first).expect("first service parses"),
    );
    assert_eq!(events.len(), 10);

    // A second instance claiming the same Agent ID would overflow the
    // directory's per-peer capacity; the tracker must drop the excess instead
    // of generating a rejection warning on every resolution and refresh tick.
    let events = tracker.observe_resolution(
        "second._axon._udp.local.".to_string(),
        parse_resolved_service(&local, &second).expect("second service parses"),
    );
    assert_eq!(events.len(), MAX_OBSERVATIONS_PER_PEER - 10);
    assert_eq!(
        tracker.refresh_events(Instant::now()).len(),
        MAX_OBSERVATIONS_PER_PEER
    );
}

#[test]
fn ipv6_link_local_addresses_are_not_candidates() {
    let (local, _) = identity(25);
    let (remote, public_key) = identity(26);
    let info = service_with_addresses(&remote, &public_key, "fe80::1,192.168.1.30");

    let observations = parse_resolved_service(&local, &info).expect("service parses");

    assert_eq!(observations.len(), 1);
    assert!(
        observations[0].endpoint.expect("endpoint").is_ipv4(),
        "unscoped IPv6 link-local endpoints are not dialable and must be dropped"
    );
}

// ---------------------------------------------------------------------------
// I12 — attacker-controlled display names
// ---------------------------------------------------------------------------

#[test]
fn display_name_control_characters_are_replaced() {
    let (local, _) = identity(27);
    let (remote, public_key) = identity(28);
    let properties = [
        ("agent_id", remote.as_str()),
        ("pubkey", public_key.as_str()),
    ];
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "bad\u{1b}[31m\nname\u{7}",
        "bad-name.local.",
        "192.168.1.40",
        7100,
        &properties[..],
    )
    .expect("service info");

    let observations = parse_resolved_service(&local, &info).expect("service parses");
    let name = observations[0]
        .display_name
        .as_deref()
        .expect("display name survives");

    assert!(
        name.chars().all(|ch| !ch.is_control()),
        "control characters must not reach tables, logs, or IPC: {name:?}"
    );
    assert_eq!(name, "bad\u{FFFD}[31m\u{FFFD}name\u{FFFD}");
}

#[test]
fn display_name_is_truncated_on_a_char_boundary() {
    let (local, _) = identity(29);
    let (remote, public_key) = identity(30);
    let properties = [
        ("agent_id", remote.as_str()),
        ("pubkey", public_key.as_str()),
    ];
    let long_name = "\u{e9}".repeat(200);
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        &long_name,
        "long-name.local.",
        "192.168.1.41",
        7100,
        &properties[..],
    )
    .expect("service info");

    let observations = parse_resolved_service(&local, &info).expect("service parses");
    let name = observations[0]
        .display_name
        .as_deref()
        .expect("display name survives");

    assert!(name.len() <= MAX_DISPLAY_NAME_BYTES);
    assert_eq!(name.chars().count(), MAX_DISPLAY_NAME_BYTES / 2);
    assert!(name.chars().all(|ch| ch == '\u{e9}'));
}
