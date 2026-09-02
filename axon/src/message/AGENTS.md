# AGENTS.md (message)

This file applies to message types and envelope code in `axon/src/message/`.

## Priorities

Spec compliance first — envelope schema must match `spec/WIRE_FORMAT.md` §6.

## File responsibilities

- `envelope.rs`: Envelope struct, MessageKind enum, encode/decode, validation, and `MAX_MESSAGE_SIZE`.
- `mod.rs`: Module exports.

## Guardrails

- Five kinds have defined v1 semantics (`request`, `response`, `message`, `error`, `describe`), but unknown kind strings must be retained exactly for forward compatibility. `describe` is answered by the receiving daemon from the manifest published at `serve` time (see `axon/src/manifest/`); it is never delivered to an application handler.
- `AgentId` values must be validated at construction; do not add unchecked string constructors.
- Unknown JSON fields must be tolerated (forward compatibility).
- `MAX_MESSAGE_SIZE` changes require README.md Configuration Reference update.

## Test targets

- Unit: `envelope_tests.rs`
- Spec compliance: `axon/tests/spec_compliance.rs`
