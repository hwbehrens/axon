# AGENTS.md (ipc)

This file applies to IPC protocol and server code in `axon/src/ipc/`.

## Priorities

Spec compliance (`spec/IPC.md`) > security > usability.

## File responsibilities

- `protocol.rs`: IPC command/reply schema, serialization.
- `server.rs`: Listener lifecycle, client accept, broadcast.
- `client_handler.rs`: Per-client command dispatch, inbound event delivery.
- `auth.rs`: Unix peer credential authentication.
- `mod.rs`: Module exports.

## Guardrails

- Command semantics must match `spec/IPC.md` §3.
- Bounded queues must overflow-disconnect lagging clients, not silently drop messages.
- Validate all inbound data before forwarding to IPC subscribers.
- `MAX_IPC_LINE_LENGTH` changes require README.md update.

## Test targets

- Unit: `server_tests.rs`
- CLI contract: `axon/tests/cli_contract.rs`
- Spec compliance: `axon/tests/spec_compliance.rs`
