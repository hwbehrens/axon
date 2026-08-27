# AGENTS.md (message)

This file applies to message types and envelope code in `axon/src/message/`.

## Priorities

Spec compliance first — envelope schema must match `spec/WIRE_FORMAT.md` §6.

## File responsibilities

- `envelope.rs`: Envelope struct, MessageKind enum, encode/decode, validation, and `MAX_MESSAGE_SIZE`.
- `mod.rs`: Module exports.

## Guardrails

- Four kinds have defined v1 semantics (`request`, `response`, `message`, `error`), but unknown kind strings must be retained exactly for forward compatibility.
- `AgentId` values must be validated at construction; do not add unchecked string constructors.
- Unknown JSON fields must be tolerated (forward compatibility).
- `MAX_MESSAGE_SIZE` changes require README.md Configuration Reference update.

## Test targets

- Unit: `envelope_tests.rs`
- Spec compliance: `axon/tests/spec_compliance.rs`
