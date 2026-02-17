# AXON Quality Evaluation (Rubric: `rubrics/QUALITY.md`)

Date: 2026-02-17  
Scope: Current working tree in `/Users/hwb/Projects/axon` (including uncommitted changes)

## Method

- Reviewed `spec/` and implementation in `axon/src/` plus integration/spec/adversarial tests.
- Ran verification:
  - `cd axon && make check`
  - `cd axon && make fmt && make lint`
  - `cd axon && make test-all`
  - `cd axon && make verify`
- Verified file-size invariant (`.rs` files under 500 lines).

## Findings and Deductions

### 1) Correctness & Protocol Behavior (`-3`)

- `-2` IPC `req_id` echo requirement is not met for parse-level invalid commands.
  - Spec requires echo when `req_id` is present (`spec/IPC.md:30`).
  - Invalid-command fast path hardcodes `req_id: None` (`axon/src/ipc/server.rs:19`, `axon/src/ipc/server.rs:27`, `axon/src/ipc/server.rs:356`, `axon/src/ipc/server.rs:378`).
- `-1` Outbound request path does not enforce that bidi replies are only `response` or `error`.
  - Request/response mapping is normative (`spec/MESSAGE_TYPES.md:11`, `spec/MESSAGE_TYPES.md:14`, `spec/WIRE_FORMAT.md:173`).
  - `send_request` validates envelope shape but does not check `kind` (`axon/src/transport/connection.rs:80`, `axon/src/transport/connection.rs:84`, `axon/src/transport/connection.rs:87`).

### 2) Security & Hardening (`-0`)

- Strong mTLS pinning and unknown-peer rejection are implemented and tested (`axon/src/transport/tls.rs:124`, `axon/src/transport/tls.rs:142`, `axon/src/transport/tls.rs:215`, `axon/src/transport/tls_tests.rs:87`).
- Socket/key permissions are explicitly tightened (`axon/src/ipc/server.rs:116`, `axon/src/identity.rs:51`, `axon/src/config.rs:62`).

### 3) Testing Quality & Coverage (`-3`)

- `-2` No regression tests asserting `req_id` echo behavior for error paths called out in IPC spec.
  - Existing IPC tests use `req_id: None` and do not assert echo semantics (`axon/tests/integration/ipc.rs:31`, `axon/tests/integration/ipc.rs:75`, `axon/tests/integration/ipc.rs:136`).
- `-1` No test that client-side request path rejects malformed reply kind in `send_request`.
  - Coverage exists for many wire violations, but not this specific client-side check (`axon/tests/integration/violations/mod.rs:109`, `axon/src/transport/connection.rs:80`).

### 4) Performance & Resource Efficiency (`-0`)

- Resource caps and bounded queues are present (`axon/src/daemon/mod.rs:16`, `axon/src/daemon/mod.rs:20`, `axon/src/ipc/server.rs:17`, `axon/src/ipc/server.rs:279`).
- Hot-path benchmarks exist for message/IPC/peer table (`axon/benches/message_bench.rs:53`, `axon/benches/ipc_bench.rs:25`, `axon/benches/peer_table_bench.rs:17`).

### 5) Reliability, Error Handling & Robustness (`-2`)

- `-2` Daemon startup fails hard if `known_peers.json` is corrupted.
  - `load_known_peers` returns parse errors (`axon/src/config.rs:126`, `axon/src/config.rs:127`).
  - Daemon propagates that error directly at startup (`axon/src/daemon/mod.rs:86`).

### 6) Maintainability & Code Health (`-2`)

- `-1` Agent-ID derivation logic is duplicated in two modules.
  - `axon/src/identity.rs:140`
  - `axon/src/transport/tls.rs:281`
- `-1` Unused complexity in CLI IPC helper: correlated-response branch is implemented but never used in call sites.
  - Call sites always pass `false` (`axon/src/main.rs:83`, `axon/src/main.rs:93`, `axon/src/main.rs:101`, `axon/src/main.rs:105`).
  - Branching logic remains in helper (`axon/src/main.rs:174`, `axon/src/main.rs:208`).

### 7) Build/CI Hygiene (`-2`)

- `-2` New fuzz target exists but is not exercised by canonical automation.
  - Target is defined: `fuzz_ipc_session` (`axon/fuzz/Cargo.toml:49`).
  - Omitted from `make fuzz` (`axon/Makefile:38` to `axon/Makefile:46`).
  - Omitted from CI fuzz loop (`.github/workflows/ci.yml:140`).

## Score Sheet

| Category | Max | Score |
|---|---:|---:|
| 1) Correctness & Protocol Behavior | 20 | 17 |
| 2) Security & Hardening | 18 | 18 |
| 3) Testing Quality & Coverage | 20 | 17 |
| 4) Performance & Resource Efficiency | 12 | 12 |
| 5) Reliability & Robustness | 10 | 8 |
| 6) Maintainability & Code Health | 12 | 10 |
| 7) Build/CI Hygiene | 8 | 6 |
| **Total** | **100** | **88** |

## Summary

The core transport, security model, and test harness are strong and currently green under full verification. Main deductions are concentrated in IPC edge-case conformance (`req_id` echo), startup robustness around corrupted peer cache, and CI/fuzz automation drift.
