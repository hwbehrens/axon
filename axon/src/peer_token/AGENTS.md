# AGENTS.md (peer_token)

This file applies to peer token encoding/decoding in `axon/src/peer_token/`.

## Priorities

Correctness > interoperability.

## File responsibilities

- `mod.rs`: `axon://<pubkey_base64url>@<host>:<port>` token format, encode/decode.

## Guardrails

- Token format is `axon://<pubkey_base64url>@<host>:<port>`. Do not change the URI scheme without spec update.
- Round-trip encode/decode must always be tested — a token produced by `encode` must parse back identically via `decode`.
- Base64url encoding (no padding) for the public key component.

## Test targets

- Unit: `tests.rs`
