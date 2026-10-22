use std::net::IpAddr;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use mdns_sd::ServiceInfo;

use super::ResolvedRecord;
use crate::discovery::{SERVICE_TYPE, parse_resolved_service};
use crate::message::AgentId;

fn advertisement(seed: u8, addresses: &str) -> (AgentId, ServiceInfo) {
    let public_key = STANDARD.encode([seed; 32]);
    let agent_id = AgentId::from_pubkey_base64(&public_key).expect("valid test key");
    let properties = [
        ("agent_id", agent_id.as_str()),
        ("pubkey", public_key.as_str()),
    ];
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "remote-agent",
        "remote-agent.local.",
        addresses,
        7100,
        &properties[..],
    )
    .expect("service info");
    (agent_id, info)
}

fn sorted_addresses(record: &impl ResolvedRecord) -> Vec<IpAddr> {
    let mut addresses = record.addresses();
    addresses.sort();
    addresses
}

/// Production browse events carry `ResolvedService`, which only `mdns-sd`
/// can construct; `ServiceInfo::as_resolved_service` is its own conversion,
/// so it exercises the production adapter without live multicast.
#[test]
fn resolved_service_adapter_matches_service_info() {
    let (_, info) = advertisement(2, "192.168.1.20,fe80::1,2001:db8::7,127.0.0.1");
    let resolved = info.clone().as_resolved_service();

    assert_eq!(resolved.fullname(), info.fullname());
    assert_eq!(resolved.port(), 7100);
    assert_eq!(resolved.property("agent_id"), info.property("agent_id"));
    assert_eq!(resolved.property("pubkey"), info.property("pubkey"));
    assert_eq!(resolved.property("missing"), None);
    assert_eq!(sorted_addresses(&resolved), sorted_addresses(&info));
    assert_eq!(resolved.addresses().len(), 4);
}

#[test]
fn resolved_service_parses_to_the_same_observations() {
    let (local, _) = advertisement(1, "192.168.1.10");
    let (remote, info) = advertisement(2, "192.168.1.20,fe80::1,2001:db8::7,127.0.0.1");

    let from_info = parse_resolved_service(&local, &info).expect("service info parses");
    let from_resolved = parse_resolved_service(&local, &info.clone().as_resolved_service())
        .expect("resolved service parses");

    let view = |observations: &[crate::peer_directory::PeerObservation]| {
        observations
            .iter()
            .map(|o| {
                (
                    o.id.clone(),
                    o.identity.clone(),
                    o.endpoint,
                    o.display_name.clone(),
                    o.source,
                )
            })
            .collect::<Vec<_>>()
    };
    // Loopback and unscoped link-local are filtered on both paths.
    assert_eq!(from_resolved.len(), 2);
    assert_eq!(view(&from_resolved), view(&from_info));
    assert!(
        from_resolved
            .iter()
            .all(|o| o.identity.agent_id() == &remote)
    );
}
