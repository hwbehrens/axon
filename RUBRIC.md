# AXON Implementation Scoring Rubric

Total: **100 points across 8 categories.**

This rubric is designed for a mature AXON implementation where the protocol is already largely complete. Scoring now prioritizes **security, test quality, performance, maintainability, and contribution quality**. Spec compliance still matters, but is evaluated mainly as **interop preservation / spec drift control**, not "did you implement everything?".

Scoring guidance (recommended):
- **Start each category at max points and deduct** for missing items, regressions, or unaddressed risks.
- Criteria are written to be **checkable** in code review and CI (tests, fuzz targets, clippy, benches, docs, etc.).
- Applies equally to **SI agent PRs** and **human PRs**.

---

## 1. Security & Hardening (max 18)
Does the change preserve or improve AXON's security posture under realistic threat models (malicious peers, hostile LAN, untrusted input, local IPC misuse)?

Check:

- **mTLS peer pinning and identity binding remain correct**
  - Certificate Ed25519 pubkey → derived `AgentId` matches `hello.from`.
  - Unknown peers are rejected during TLS verification unless present in expected peer table (discovery/static/cache).
  - SNI uses typed agent ID (`ed25519.<32hex>`) for outbound connections.

- **Handshake and authorization gates remain enforced**
  - "Hello must be first": pre-hello **uni** messages are dropped; pre-hello **bidi** non-hello requests get `error(not_authorized)` (per spec).
  - Post-hello: envelope validation occurs before forwarding to IPC / handlers.

- **Replay and duplicate handling remains enforced**
  - Replay cache dedup works (TTL-based) and remains bounded.
  - Replay behavior is consistent for all inbound paths (uni + bidi) that can reach IPC.

- **Resource/DoS controls are preserved**
  - Max message size enforced (64KiB).
  - IPC line length and per-client queues remain bounded; drop/backpressure behavior is explicit.
  - Connection caps / handshake deadlines remain in place for unauthenticated inbound peers.

- **Secrets and sensitive data are protected**
  - Never logs private key material, raw key bytes, or sensitive payloads by default.
  - Filesystem permissions remain strict (identity key `0600`, config dir `0700`, socket `0600`).
  - No "debug shortcuts" left enabled (e.g., disabling verification, accepting unknown peers).

Typical deductions:
- Weakening TLS verification/pinning, removing gating, forwarding unvalidated envelopes, unbounded queues, leaking sensitive info in logs, adding new attack surface without tests.

---

## 2. Test Quality & Coverage (max 18)
Do tests *meaningfully* verify behavior, invariants, and regressions—not just execute code paths?

Check:

- **Tests added/updated for every behavior change**
  - Unit tests cover new public behavior and error paths.
  - Integration tests cover cross-module flows when applicable (IPC → daemon → transport, discovery → pinning → connect, reconnect loops, etc.).

- **Invariant-driven testing**
  - Tests explicitly assert AXON invariants: hello gating, peer pinning, initiator rule (lower agent_id initiates), replay dedup, max size enforcement, stream mapping expectations.
  - Serde round-trips verify envelope/payload stability, including forward-compat handling (unknown fields tolerated).

- **Spec compliance tests remain green**
  - `axon/tests/spec_compliance.rs` updated when schemas/wire behavior changes.
  - No "silent drift": behavior changes that impact interoperability must be reflected in tests and/or spec docs.

- **Property-based testing where it pays off**
  - If change touches parsing/validation/state machines: add or extend `proptest` coverage.
  - Any generated `proptest-regressions/` files are committed (not ignored) when produced.

- **No flaky or timing-dependent tests**
  - Timeouts use deterministic control where feasible (Tokio time pausing if used in repo).
  - Tests avoid "sleep and pray" patterns; use signals/events.

Typical deductions:
- Tests that only check "it doesn't crash", lack assertions on invariants, missing regression tests for bug fixes, or changes without tests.

---

## 3. Performance & Efficiency (max 14)
Does the change preserve AXON's "fast and light" goals (low idle CPU, low memory, low latency), without premature micro-optimizations?

Check:

- **No obvious performance regressions**
  - Avoid unnecessary allocations/copies in hot paths (framing, envelope decode/encode, routing).
  - Avoid repeatedly parsing JSON if raw preservation (`RawValue`) or structured types already exist.

- **Async correctness that impacts performance**
  - No blocking file I/O on the async reactor (use `tokio::fs` or `spawn_blocking` where appropriate).
  - No accidental blocking locks in async paths unless justified (e.g., rustls verifier callbacks).

- **Bounded concurrency and backpressure are maintained**
  - Connection limits / semaphores remain enforced.
  - IPC queues remain bounded with explicit drop policy.

- **Benchmarks are updated when relevant**
  - If change touches a known hot path (wire/framing/envelope validation/replay cache), update or add Criterion benches in `axon/benches/` where practical.

Typical deductions:
- Introducing blocking I/O in the daemon loop, unbounded buffers, lock contention, "pretty JSON" on the wire, or removing caps/timeouts.

---

## 4. Maintainability & Code Quality (max 14)
Will the code still be easy to change safely a year from now?

Check:

- **Idiomatic, readable Rust**
  - Clear types over "stringly-typed" JSON construction (prefer typed payload structs over ad-hoc `json!()` where it prevents schema mistakes).
  - Errors use consistent patterns (`anyhow::Context` or typed errors) and preserve actionable context.

- **Module boundaries and file constraints**
  - New code fits the existing module map (message/transport/daemon/ipc/discovery/etc.).
  - Source files stay under the repo constraint (≤ 500 lines); split modules when approaching the limit.

- **Minimal and justified complexity**
  - No new dependencies unless clearly necessary and aligned with repo conventions.
  - No broad refactors mixed into functional changes without strong justification.

- **Clarity of invariants**
  - If a change affects a "load-bearing invariant" (hello gating, pinning, initiator rule, replay), it is encoded in types/tests and documented where the repo expects it (often tests + spec, not long comments).

Typical deductions:
- Ad-hoc schemas, duplicated logic, unclear lifetimes/ownership flows, inconsistent error handling, oversized files, or "clever" patterns that reduce auditability.

---

## 5. Operational Maturity (max 10)
Does AXON remain operable as a long-running daemon in real environments?

Check:

- **Observability is actionable**
  - `tracing` events/spans include peer identifiers (agent_id), message IDs (UUID), and connection lifecycle transitions.
  - Logs are not excessively noisy under adversarial conditions (protocol violations should not spam at high levels).

- **Configurability and sane defaults**
  - Operational knobs (timeouts, reconnect backoff, limits) are configurable where the spec/repo expects, or intentionally constant with rationale.
  - Config precedence remains correct (CLI vs config.toml vs defaults), if modified.

- **Graceful degradation**
  - If a peer lacks a feature (hello `features`), behavior fails gracefully (don't send unsupported kinds; produce useful errors).
  - Daemon startup/shutdown remains clean: closes connections, persists state, removes stale socket.

Typical deductions:
- Poor/no logging for failures, breaking daemon lifecycle, hard-coded operational knobs added without reason, or noisy logs that become a DoS vector.

---

## 6. Adversarial Robustness (max 10)
How well does the code handle malicious or malformed inputs and protocol abuse?

Check:

- **Fuzzing coverage is kept current**
  - New deserialization/parse entrypoints get a fuzz target in `axon/fuzz/fuzz_targets/`.
  - Fuzz targets remain stable and useful (not trivially rejecting all inputs).

- **Adversarial test suite is extended when needed**
  - Add/extend adversarial tests for: malformed JSON, oversized frames, stream resets, unknown kinds, wrong stream type, pre-hello messages, invalid `from/to`, replay duplicates, corrupted IPC commands.

- **Protocol violation handling remains safe**
  - Invalid frames are dropped without panics.
  - Bidi paths respond with `error(...)` where required; uni paths drop where required.
  - No path forwards malformed/unvalidated envelopes to IPC subscribers.

- **Mutation testing friendliness (where used)**
  - Changes that reduce test sensitivity (e.g., removing assertions) are avoided.
  - If the repo's `cargo-mutants` flows are touched, keep them passing or update configs intentionally.

Typical deductions:
- Missing fuzz target for new parser surface, forwarding malformed messages, panics on bad input, or removing/weakening adversarial tests.

---

## 7. Contribution Hygiene (max 10)
Does the PR look like it belongs in a mature, CI-driven Rust project?

Check:

- **Local verification matches CONTRIBUTING/Makefile expectations**
  - `cargo fmt`, `cargo clippy -- -D warnings`, and full test suite pass (or `make verify` equivalent).
  - No new warnings; no ignored tests added without explicit justification.

- **PR contains the "whole change"**
  - Tests included.
  - Spec/docs updated when behavior or schemas change (`spec/*.md`, `AGENTS.md`, `CONTRIBUTING.md` if required).
  - If behavior changes impact wire compatibility: spec + spec compliance tests updated in the same PR.

- **Small, reviewable, and focused**
  - Avoid drive-by refactors.
  - Commit message/PR description clearly states: what changed, why, and any security/perf implications.

Typical deductions:
- CI failures, missing tests, undocumented behavior drift, unrelated refactors, or changes that violate repo conventions.

---

## 8. Interop & Spec Drift Control (max 6)
AXON's spec is implemented; this category ensures changes don't silently break interoperability.

Check:

- **Wire compatibility preserved**
  - One-message-per-QUIC-stream with FIN delimiting remains unchanged.
  - Max message size and JSON encoding rules remain compliant.
  - Stream mapping deviations are handled as specified (and tested).

- **Schema changes are managed correctly**
  - Envelope fields remain stable; unknown fields continue to be ignored (forward compatibility).
  - Payload changes update: `spec/MESSAGE_TYPES.md`, `spec/WIRE_FORMAT.md`, plus spec compliance tests.

- **No "implementation-only protocol"**
  - If behavior changes would affect another-language implementation, the spec documents it.

Typical deductions:
- Breaking wire format, changing envelope fields without spec+tests, or introducing undocumented interop behavior.

---

## Summary Table

| Category | Max | agent/codex | agent/amp | agent/claude | agent/gemini |
|---|---|---|---|---|---|
| 1. Security & Hardening | 18 |  |  |  |  |
| 2. Test Quality & Coverage | 18 |  |  |  |  |
| 3. Performance & Efficiency | 14 |  |  |  |  |
| 4. Maintainability & Code Quality | 14 |  |  |  |  |
| 5. Operational Maturity | 10 |  |  |  |  |
| 6. Adversarial Robustness | 10 |  |  |  |  |
| 7. Contribution Hygiene | 10 |  |  |  |  |
| 8. Interop & Spec Drift Control | 6 |  |  |  |  |
| **Total** | **100** | **/100** | **/100** | **/100** | **/100** |
