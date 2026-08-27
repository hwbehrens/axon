# AGENTS.md (daemon)

This file applies to daemon orchestration code in `axon/src/daemon/`.

## Priorities

Reliability > correctness > simplicity. The daemon is a lightweight router — no protocol logic here.

## File responsibilities

- `mod.rs`: Event loop, startup/shutdown, candidate observation, and resource bounds (`MAX_CONNECTIONS`, `KEEPALIVE`, `IDLE_TIMEOUT`, `MAX_IPC_CLIENTS`, `MAX_CLIENT_QUEUE`, `MAX_INFLIGHT_SENDS`).
- `command_handler.rs`: IPC command dispatch to appropriate handlers.
- `lockfile.rs`: PID file management for single-instance enforcement.

## Guardrails

- Do not embed protocol logic (message routing rules, envelope validation) in the daemon — that belongs in `transport/` or `message/`.
- Maintain bounded resource usage; all constants changes require README.md update.
- The daemon coordinates owners; peer truth belongs to `PeerDirectory`, request state to `RequestBroker`, and connection/reconnect state to `ConnectionManager`.
- Background work must have an explicit cancellation path and be joined during bounded shutdown.
- Lockfile semantics must prevent concurrent daemon instances.

## Test targets

- Unit: `lockfile_tests.rs`
- E2E: `axon/tests/daemon_lifecycle.rs`
- Integration: `axon/tests/integration.rs`
