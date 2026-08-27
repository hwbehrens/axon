# AGENTS.md (request_broker)

This file applies to inbound request ownership in `axon/src/request_broker/`.

## Priorities

Exactly one terminal outcome > bounded state > handler convenience.

## File responsibilities

- `mod.rs`: single handler lease, pending request correlation, reply authorization, disconnect/failure completion.

## Guardrails

- At most one IPC client may hold the request-handler lease.
- A request is completed exactly once by its owning handler or an explicit timeout/disconnect/unhandled error.
- Responses return on the original QUIC bidirectional stream; do not create a second outbound message.
- Pending state and handler queues must remain bounded.
- Request correlation is scoped to `(authenticated remote AgentId, request UUID)`; outcomes are never replayed across peers. A reply whose UUID matches several peer-scoped pending requests without a `peer` fails as ambiguous.
- Unknown or duplicate replies fail explicitly and never mutate unrelated requests.

## Test targets

- Unit: module-local tests in `mod.rs`
- Integration/spec: `axon/tests/integration.rs`, `axon/tests/spec_compliance.rs`
