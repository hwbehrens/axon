use base64::{Engine as _, engine::general_purpose::STANDARD};
use mdns_sd::ServiceInfo;

use super::*;

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
