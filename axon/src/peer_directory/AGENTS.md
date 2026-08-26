# AGENTS.md (peer_directory)

This file applies to logical peer ownership in `axon/src/peer_directory/`.

## Priorities

Identity and trust invariants > one authoritative owner > availability.

## File responsibilities

- `mod.rs`: `PeerDirectory` transitions, bounded candidates/enrolled peers, derived views and pinning snapshots.
- `state.rs`: internal peer/observation state and derived representations.
- `types.rs`: validated identities, observations, locators, trust and conflict representations.
- `store.rs`: versioned `peers.json` schema and atomic whole-file persistence.
- `persistence.rs`: the transaction worker (snapshot → lock-free save → generation-checked apply) shared by every persistent edit.

## Guardrails

- `PeerDirectory` is the sole live owner of identity, trust, locators, observations, and conflicts.
- Discovery adds candidates only; explicit enrollment is the only path into the TLS pin set.
- Persist enrolled intent and configured locators only. Never persist mDNS liveness or observed addresses.
- Validate and durably persist a mutation before publishing its new immutable pinning snapshot.
- Peer-store I/O never runs under the state lock: each persistent edit is ONE serialized transaction — the save gate is held across build, save, and apply, so no generation races, retries, or heal paths exist (DEC-021/022/023); the save gate orders gate -> state lock only.
- save-then-apply runs on an owned transaction worker, so caller cancellation can never leave disk ahead of memory; `store.save` never errors after its rename (post-rename sync failures are warnings).
- `observation_index` must stay ghost-free: every entry resolves to a live enrolled/candidate observation, and revocation removes the record's entire observation set at commit time (pinned by Hegel invariants and interleaving tests).
- Conflicting identity/address evidence is quarantined, never resolved by last-writer-wins.
- Keep all peer, locator, and observation collections bounded; constant changes require README.md updates.

## Test targets

- Unit: `tests.rs` plus module-local tests where needed
- Property: `properties.rs` (store roundtrip/bounds, proptest) and `state_machine.rs` (Hegel stateful rules + trust invariants; see DEC-015)
- Integration/adversarial: `axon/tests/integration.rs`, `axon/tests/adversarial.rs`
- Fuzz: `axon/fuzz/fuzz_targets/fuzz_peer_store.rs` exercises the untrusted `peers.json` decode path
