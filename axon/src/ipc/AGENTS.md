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
- EVERY outbound line passes `encode_reply_line`/`error_reply_line`, which enforce the 65,536-byte limit including the trailing newline: oversized replies fail explicitly with `message_too_large`, oversized broadcasts are dropped with a warning, oversized handler deliveries fail to the broker's terminal-error path — never truncate (DEC-022/DEC-023). The client-handler's malformed-command replies go through the same encoder.
- `req_id` is bounded at ingress (`MAX_REQ_ID_BYTES`); overlong values are rejected with `invalid_command` and never echoed — an unbounded echo could make any reply frame exceed the limit.
- Bounded queues must overflow-disconnect lagging clients, not silently drop messages.
- Validate all inbound data before forwarding to IPC subscribers.
- `add_peer`/`remove_peer` are the only runtime trust mutation boundary.
- `serve` leases the single request-handler role; `reply` completes only requests owned by that client.
- The server owns and joins its accept/client tasks during bounded shutdown.
- `MAX_IPC_LINE_LENGTH` changes require README.md update.

## Test targets

- Unit: `server_tests.rs`
- CLI contract: `axon/tests/cli_contract.rs`
- Spec compliance: `axon/tests/spec_compliance.rs`
