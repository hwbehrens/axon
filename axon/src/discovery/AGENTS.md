# AGENTS.md (discovery)

This file applies to peer discovery code in `axon/src/discovery/`.

## Priorities

Zero-config LAN operation > correctness > extensibility.

## File responsibilities

- `mod.rs`: mDNS service registration/browsing, static peer loading, discovery event types.

## Guardrails

- mDNS service type is `_axon._udp.local.` — do not change without spec update.
- TXT record format is normative (`spec/WIRE_FORMAT.md` §11.2).
- Discovered peers must go through PeerTable for pinning before connections are accepted.
- Static peers (from config) take precedence over mDNS-discovered peers at the same address.

## Test targets

- Unit: `tests.rs`
- Integration: `axon/tests/integration.rs`
