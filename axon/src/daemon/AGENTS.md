# AGENTS.md (daemon)

This file applies to daemon orchestration code in `axon/src/daemon/`.

## Priorities

Reliability > correctness > simplicity. The daemon is a lightweight router — no protocol logic here.

## File responsibilities

- `mod.rs`: Event loop, startup/shutdown, resource bounds (`MAX_CONNECTIONS`, `KEEPALIVE`, `IDLE_TIMEOUT`, `MAX_IPC_CLIENTS`, `MAX_CLIENT_QUEUE`).
- `command_handler.rs`: IPC command dispatch to appropriate handlers.
- `peer_events.rs`: Discovery event handling, peer table updates.
- `reconnect.rs`: Reconnection logic with exponential backoff.
- `lockfile.rs`: PID file management for single-instance enforcement.

## Guardrails

- Do not embed protocol logic (message routing rules, envelope validation) in the daemon — that belongs in `transport/` or `message/`.
- Maintain bounded resource usage; all constants changes require README.md update.
- Reconnect backoff (1s initial, doubling to 30s max) must be preserved.
- Lockfile semantics must prevent concurrent daemon instances.

## Test targets

- Unit: `reconnect_tests.rs`, `lockfile_tests.rs`
- E2E: `axon/tests/daemon_lifecycle.rs`
- Integration: `axon/tests/integration.rs`
