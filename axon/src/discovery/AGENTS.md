# AGENTS.md (discovery)

This file applies to peer discovery code in `axon/src/discovery/`.

## Priorities

Correct identity observations > zero-config LAN discovery > extensibility.

## File responsibilities

- `mod.rs`: Bonjour/mDNS service registration, browsing, validation, and observation events.

## Guardrails

- mDNS service type is `_axon._udp.local.` — do not change without spec update.
- TXT record format is normative (`spec/WIRE_FORMAT.md` §11.2).
- Discovery produces untrusted `PeerObservation` candidates only. It must never mutate TLS trust directly.
- Agent ID must derive from the advertised Ed25519 public key before an observation reaches `PeerDirectory`.
- Lost events remove only their observation; they do not revoke enrolled trust.
- WAN discovery and static peer configuration are out of scope.

## Test targets

- Unit: `tests.rs`
- Integration: `axon/tests/integration.rs`
- Live host-network check (ignored by default): `axon/tests/mdns_live.rs`
