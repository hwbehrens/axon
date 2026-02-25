# AXON — QUALITY Scoring Rubric (Code Quality)

Status: Normative

Total: **100 points** across 7 categories.

This rubric evaluates **engineering quality**: correctness, safety, tests, performance, reliability, and maintainability.

Spec files are authoritative: `spec/SPEC.md`, `spec/WIRE_FORMAT.md`, `spec/MESSAGE_TYPES.md`, `spec/IPC.md`.
Use alongside:
- `ALIGNMENT.md` (LLM-first / project philosophy fit)
- `DOCUMENTATION.md` (spec/README/docs quality)

## Evaluation principles

Apply [`EVALUATION-PRINCIPLES.md`](EVALUATION-PRINCIPLES.md), especially material-impact scoring and shared severity calibration.

## Scoring method
- **Start each category at max points and deduct** for missing items, regressions, or unaddressed risks.
- Criteria should be **checkable** in review and CI (tests, clippy, fuzz, benches, invariants).
- Applies to human and agent contributions.

---

## 1) Correctness & Protocol Behavior (max 20)
Does the change do the right thing, including edge cases, without breaking required protocol behavior?

**Check (deduct if missing/regressed):**
- **Spec-defined invariants remain correct**
  - Consult `spec/` plus invariant summaries in `AGENTS.md` and `CONTRIBUTING.md` for the current invariant list.
- **Wire-level rules remain correct**
  - Envelope fields, framing, limits, and validation order match `spec/WIRE_FORMAT.md`.
  - One-message-per-stream behavior remains correct where specified.
- **Message behavior remains correct**
  - Message kinds and stream mapping match `spec/MESSAGE_TYPES.md`.
- **IPC behavior remains correct**
  - Command semantics and transport guarantees match `spec/IPC.md`.
- **CLI behavior contracts remain correct (when CLI is touched)**
  - Root/path selection precedence and command coherence are preserved (`--state-root`/aliases, `AXON_ROOT`, default root).
  - Exit-code semantics remain script-safe (`0` success, `1` local/runtime failure, `2` daemon/application failure reply).
- **Forward compatibility behavior preserved**
  - Unknown JSON fields are ignored where required by the spec.

**Typical deductions**
- Behavior differs from spec without deliberate spec update; identity or routing invariants broken; malformed data forwarded before validation.

---

## 2) Security & Hardening (max 18)
Does the change preserve or improve the security posture under realistic threat models (malicious peers, hostile LAN, untrusted input, local IPC misuse)?

**Check:**
- **mTLS pinning and identity verification remain enforced**
  - Outbound SNI uses full typed agent ID (`ed25519.<hex>`).
  - Unknown inbound peers are rejected during TLS verification unless in expected peer table.
  - Expected-pubkey match is enforced when pinned.
- **Input validation + safe failure**
  - Malformed frames/JSON do not panic; invalid envelopes are rejected or dropped per spec.
  - Unvalidated envelopes are not forwarded to IPC subscribers.
- **Resource/DoS controls preserved**
  - Bounded queues, bounded buffers, explicit drop/backpressure behavior.
  - Connection lifecycle limits and timeouts remain aligned with spec and code constants.
- **Sensitive data protection**
  - No logging of private keys, raw key material, tokens, or sensitive payloads by default.
  - File/socket permissions remain strict (`0600` key/token/socket, `0700` dirs as applicable).
  - No debug bypass left enabled.

**Typical deductions**
- Weakening TLS verification or pinning; unbounded buffers; leaking secrets to logs; new attack surface without tests.

---

## 3) Testing Quality & Coverage (max 20)
Do tests *meaningfully verify behavior* and protect against regression?

**Check:**
- **Every behavior change has tests**
  - Unit tests cover new/changed functions and error paths.
  - Integration tests cover cross-module flows (IPC ↔ daemon ↔ transport, discovery ↔ pinning ↔ connect, reconnect loops).
- **CLI-surface changes include black-box CLI tests**
  - `axon/tests/cli_contract.rs` includes coverage for changed flags, parsing, output, and exit semantics.
- **Invariant-driven assertions**
  - Tests explicitly assert invariants defined in `AGENTS.md`, `CONTRIBUTING.md`, and the spec files.
- **Spec compliance tests updated when relevant**
  - If wire behavior, message behavior, or IPC behavior changes, update spec-compliance coverage.
- **Property testing where it pays off**
  - Parsing, validation, and state-machine paths add or extend `proptest` coverage.
  - Commit generated `proptest-regressions/` when created.
- **No flakiness**
  - Avoid timing-only assertions; use signals/events and deterministic control where available.

**Typical deductions**
- Missing regression tests; tests that only assert "no crash"; timing-dependent flakes; no spec-compliance updates when needed.

---

## 4) Performance & Resource Efficiency (max 12)
Does the change preserve AXON's "fast and light" goals (<5MB RSS, negligible idle CPU), without premature micro-optimization?

**Check:**
- No obvious hot-path regressions (framing, envelope encode/decode, routing).
- No blocking operations on async runtime (`tokio::fs` / `spawn_blocking` where warranted).
- Resource limits remain bounded (buffers/queues, connection limits, in-memory tracking maps).
- Add/update benchmarks in `axon/benches/` when changing known hot paths (where practical).

**Typical deductions**
- Blocking I/O in async paths; unbounded allocations; noisy per-message logging; removing caps or timeouts.

---

## 5) Reliability, Error Handling & Robustness (max 10)
Does the daemon behave predictably under failures and malformed inputs?

**Check:**
- Errors are actionable and consistent (use established patterns; preserve context).
- Connection lifecycle remains robust (reconnect/backoff behavior not broken).
- Protocol-violation handling matches spec (drop vs respond with `error` depending on stream type).
- No panics in normal bad-input conditions.

**Typical deductions**
- Panics on malformed input; swallowed errors; inconsistent drop/respond behavior; unreliable reconnect.

---

## 6) Maintainability & Code Health (max 12)
Will the code remain easy to change safely?

**Check:**
- Clear, idiomatic Rust; avoids clever patterns that reduce auditability.
- Consistent error types/contexts.
- Minimal duplication; shared logic centralized appropriately.
- Changes fit existing module boundaries (message/transport/daemon/ipc/discovery/etc.).
- **File size constraint honored**: Rust source files stay <= 500 lines; split modules when approaching.

**Typical deductions**
- Oversized files; ad-hoc JSON construction where typed structs prevent mistakes; broad refactors mixed with functional changes.

---

## 7) Build/CI Hygiene (max 8)
Does the change integrate cleanly with the repo workflow?

**Check:**
- `cargo fmt`, `cargo clippy -- -D warnings`, and full tests pass (or `make verify` in `axon/`).
- No new warnings; no ignored tests without explicit justification.
- No accidental debug artifacts; no dead code; no TODO left behind in critical paths.

**Typical deductions**
- CI failures; lint warnings; partial changes without corresponding tests/docs.

---

## Score Sheet
| Category | Max | Score |
|---|---:|---:|
| 1) Correctness & Protocol Behavior | 20 | /20 |
| 2) Security & Hardening | 18 | /18 |
| 3) Testing Quality & Coverage | 20 | /20 |
| 4) Performance & Resource Efficiency | 12 | /12 |
| 5) Reliability & Robustness | 10 | /10 |
| 6) Maintainability & Code Health | 12 | /12 |
| 7) Build/CI Hygiene | 8 | /8 |
| **Total** | **100** | **/100** |
