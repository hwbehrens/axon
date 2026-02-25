# AGENTS.md (peer_table)

This file applies to peer table and pinning code in `axon/src/peer_table/`.

## Priorities

Correctness (pinning invariant) > address uniqueness > performance.

## File responsibilities

- `mod.rs`: PeerTable struct, PubkeyMap (shared with TLS verifiers), upsert/remove/query operations.

## Guardrails

- PeerTable owns the PubkeyMap — TLS verifiers read from it. No manual sync required or allowed.
- At most one non-static peer per network address; stale entries are evicted when a new identity appears at the same address.
- Static peers block discovered/cached peers from inserting at the same address.
- `STALE_TIMEOUT` changes require README.md update.

## Test targets

- Unit: `tests/basic.rs`, `tests/eviction.rs`
- Property: `tests/proptest.rs`
