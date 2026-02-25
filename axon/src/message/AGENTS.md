# AGENTS.md (message)

This file applies to message types and envelope code in `axon/src/message/`.

## Priorities

Spec compliance first — envelope schema must match `spec/WIRE_FORMAT.md` §6.

## File responsibilities

- `envelope.rs`: Envelope struct, MessageKind enum, encode/decode, validation.
- `mod.rs`: Module exports, `MAX_MESSAGE_SIZE` constant.

## Guardrails

- 4 message kinds are fixed at the protocol level (`request`, `response`, `message`, `error`). Do not add new kinds without updating `spec/MESSAGE_TYPES.md`.
- Unknown JSON fields must be tolerated (forward compatibility).
- `MAX_MESSAGE_SIZE` changes require README.md Configuration Reference update.

## Test targets

- Unit: `envelope_tests.rs`
- Spec compliance: `axon/tests/spec_compliance.rs`
