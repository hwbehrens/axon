# AGENTS.md (identity)

This file applies to identity and key management code in `axon/src/identity/`.

## Priorities

Security > correctness > simplicity.

## File responsibilities

- `mod.rs`: Ed25519 keypair generation, agent ID derivation (first 16 bytes of SHA-256 of pubkey), key file I/O.

## Guardrails

- Ed25519 only — do not add alternative key types without spec update.
- Agent ID = `ed25519.` + the first 16 bytes of `SHA-256(pubkey)`, formatted as 32 lowercase hex characters. This is a load-bearing invariant.
- `identity.key` is base64-encoded 32-byte seed. Reject non-base64 or legacy raw formats.
- Never log or expose private key material.

## Test targets

- Unit: `tests.rs`
- Integration: `axon/tests/integration.rs`
