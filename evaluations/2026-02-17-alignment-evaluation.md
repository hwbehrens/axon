# AXON Alignment Evaluation (Rubric: `rubrics/ALIGNMENT.md`)

Date: 2026-02-17  
Scope: Current working tree in `/Users/hwb/Projects/axon` (including uncommitted changes)

## Method

- Evaluated the implementation against stated AXON principles: spec-first behavior, simplicity, LLM navigability, bounded operation, and lightweight runtime.
- Cross-checked docs/spec claims with code, tests, and automation (`Makefile`, CI workflow, fuzz targets).

## Findings and Deductions

### 1) Spec-First & Interop Mindset (`-4`)

- `-2` IPC `req_id` echo contract is not fully upheld for parse-level invalid commands.
  - Spec contract: `spec/IPC.md:30`
  - Implementation fixed `req_id: None` for invalid parse path: `axon/src/ipc/server.rs:19`, `axon/src/ipc/server.rs:27`, `axon/src/ipc/server.rs:378`
- `-2` Documentation drift on CLI/IPC behavior weakens spec-first posture.
  - Spec claim: `spec/SPEC.md:201`
  - Current implementation for `identity`: `axon/src/main.rs:108`

### 2) Simplicity / YAGNI / Minimal Dependencies (`-2`)

- `-2` Unused correlated-response logic in CLI IPC helper adds complexity without current feature usage.
  - All call sites pass `false`: `axon/src/main.rs:83`, `axon/src/main.rs:93`, `axon/src/main.rs:101`, `axon/src/main.rs:105`
  - Extra branch remains: `axon/src/main.rs:208`

### 3) LLM-First Navigability & Learnability (`-2`)

- `-1` Duplicate agent-id derivation logic increases mental overhead and drift risk.
  - `axon/src/identity.rs:140`
  - `axon/src/transport/tls.rs:281`
- `-1` IPC `req_id` behavior is documented but not directly covered by integration tests, which makes rule confidence lower for future agents changing IPC parser paths.
  - Existing tests mostly use `req_id: None`: `axon/tests/integration/ipc.rs:31`, `axon/tests/integration/ipc.rs:75`

### 4) Architectural Coherence & Invariants (`-2`)

- `-2` Corrupted `known_peers.json` can prevent daemon boot, which conflicts with “run indefinitely/predictably” philosophy.
  - Parse error surface: `axon/src/config.rs:126`
  - Startup propagation: `axon/src/daemon/mod.rs:86`

### 5) Efficiency & Context-Budget Awareness (`-1`)

- `-1` Reconnect failure path logs warnings repeatedly per failing peer during churn; bounded but potentially noisy in prolonged outage environments.
  - Warning path: `axon/src/daemon/reconnect.rs:74`
  - Loop cadence: `axon/src/daemon/mod.rs:193`, `axon/src/daemon/mod.rs:252`

### 6) Operational Philosophy Fit (`-2`)

- `-2` Fuzz surface expanded (`fuzz_ipc_session`) but canonical automation does not execute it, reducing operational confidence.
  - Target exists: `axon/fuzz/Cargo.toml:49`
  - Missing from `make fuzz`: `axon/Makefile:38`
  - Missing from CI fuzz list: `.github/workflows/ci.yml:140`

## Score Sheet

| Category | Max | Score |
|---|---:|---:|
| 1) Spec-First & Interop Mindset | 20 | 16 |
| 2) Simplicity / YAGNI / Minimal Dependencies | 18 | 16 |
| 3) LLM-First Navigability & Learnability | 22 | 20 |
| 4) Architectural Coherence & Invariants | 18 | 16 |
| 5) Efficiency & Context-Budget Awareness | 12 | 11 |
| 6) Operational Philosophy Fit | 10 | 8 |
| **Total** | **100** | **87** |

## Summary

The codebase is largely aligned with AXON’s LLM-first and lightweight principles (bounded resources, clear module boundaries, strong transport/auth model). Main alignment deductions are around spec/implementation drift on IPC details, a few avoidable complexity points, and automation not yet covering the full fuzz target set.
