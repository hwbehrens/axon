# AGENTS.md (discovery)

This file applies to peer discovery code in `axon/src/discovery/`.

## Priorities

Correct identity observations > zero-config LAN discovery > extensibility.

## File responsibilities

- `mod.rs`: Bonjour/mDNS service registration, browsing, validation, and observation events.
- `record.rs`: `ResolvedRecord` adapter so parsing accepts both `ServiceInfo`
  (built locally, used by tests) and `ResolvedService` (browse results).

## Guardrails

- mDNS service type is `_axon._udp.local.` — do not change without spec update.
- TXT record format is normative (`spec/WIRE_FORMAT.md` §11.2).
- Discovery produces untrusted `PeerObservation` candidates only. It must never mutate TLS trust directly.
- Agent ID must derive from the advertised Ed25519 public key before an observation reaches `PeerDirectory`.
- Lost events remove only their observation; they do not revoke enrolled trust.
- WAN discovery and static peer configuration are out of scope.
- Instance names are attacker-controlled: sanitize control characters and bound length before a display name reaches tables, logs, or IPC.

## Dependency behavior (mdns-sd 0.21.5)

- `ServiceEvent::ServiceResolved` carries a `Box<ResolvedService>`; addresses
  arrive as `ScopedIp` values (`to_ip_addr()` drops the interface scope).
  Parse through `record::ResolvedRecord` instead of matching the concrete
  record type, so the local `ServiceInfo` path used by tests keeps working.

- `ServiceResolved` fires only for **new or changed** records. A live peer that
  keeps re-announcing unchanged records emits nothing, so the browse loop
  re-emits its cached observation set every `MDNS_REFRESH_INTERVAL` (30s) with
  a fresh `observed_at`. That interval MUST stay below the peer directory's
  `OBSERVATION_STALE_TIMEOUT` (60s); otherwise live peers expire. Do not
  remove the refresh without replacing it with another liveness source.
- `ServiceRemoved` maps to `Lost`. It is emitted when the instance's PTR
  record expires, when its last SRV record expires, and when a re-resolved
  instance becomes invalid (empty host or no addresses). The last case is new
  since 0.11.5, which kept such an instance and re-queried it instead, so a
  peer can now drop out whole until its records resolve again.
- The dependency's DNS record cache is unbounded and not inspectable, so AXON
  bounds only what it tracks and forwards: `MAX_TRACKED_SERVICES` services
  and at most `MAX_OBSERVATIONS_PER_PEER` live observations per Agent ID.
- Teardown must call `unregister` (multicasts the goodbye packet) and await
  its acknowledgement before `shutdown`, so peers learn about withdrawal
  promptly instead of waiting for TTL expiry.

## Test targets

- Unit: `tests.rs` (includes fake-clock refresh, budget, and sanitizer tests)
- Integration: `axon/tests/integration.rs`
- Live host-network check (ignored by default): `axon/tests/mdns_live.rs`
