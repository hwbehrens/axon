# AXON — QUALITY Scoring Rubric (Code Quality)
Total: **100 points** across 7 categories.

This rubric evaluates **engineering quality**: correctness, safety, tests, performance, reliability, and maintainability.
Use alongside:
- `ALIGNMENT.md` (LLM-first / project philosophy fit)
- `DOCUMENTATION.md` (spec/README/docs quality)

## Evaluation principles

You are an impartial, rigorous technical reviewer. Follow these principles:

- **Fair and evidence-based.** Every deduction must cite a concrete, verifiable signal in the diff, code, spec, or documentation — never penalize on vague intuition. Equally, never award points on good intentions; verify the actual artifact.
- **First-principles thinking.** Evaluate what the change *actually does*, not what the commit message claims. Read the code; read the spec; check that they agree. If they disagree, that is a finding.
- **100 means flawless.** A perfect score in any category means you examined every applicable check, found zero issues, and would stake your reputation on it. Do not round up. If in doubt, deduct — the author can rebut.
- **This is not a rubber stamp.** Assume the change has defects until proven otherwise. Actively look for: correctness bugs, missing edge-case tests, security regressions, resource leaks, unbounded allocations, panic paths, protocol violations, and dead code.
- **Thorough, not cursory.** Read the actual files — not just the diff summary. Trace error paths. Check that test assertions match spec requirements. Verify resource bounds are enforced. Look for concurrency issues.
- **Deductions are cumulative and specific.** State the category, the issue, the evidence (file + line or spec section), and the point cost. One issue may cause deductions in multiple categories if it violates multiple rubric checks.
- **Proportional severity.** A correctness bug or security regression warrants a larger deduction than a style nit. Use judgment, but always explain the reasoning.

## Scoring method
- **Start each category at max points and deduct** for missing items, regressions, or unaddressed risks.
- Criteria should be **checkable** in review and CI (tests, clippy, fuzz, benches, invariants).
- Applies to human and agent contributions.

---

## 1) Correctness & Protocol Behavior (max 20)
Does the change do the right thing, including edge cases, without breaking required protocol behavior?

**Check (deduct if missing/regressed):**
- **Spec-defined invariants remain correct**
  - *Hello gating*: before successful QUIC `hello`:
    - unauthenticated **uni** messages are dropped (not forwarded to IPC)
    - unauthenticated **bidi** non-`hello` requests get `error(not_authorized)`
  - *Agent ID binding*: `hello.from` matches ID derived from the peer certificate public key.
  - *Initiator rule*: lower lexicographic `agent_id` initiates; higher-ID waits briefly then errors as implemented.
  - *Replay dedup*: duplicates (same envelope UUID) are dropped within TTL; cache stays bounded.
- **Wire-level rules remain correct**
  - One-message-per-QUIC-stream, FIN delimits message (no length prefix).
  - Max message size enforcement remains correct (64KiB baseline).
  - Envelope parsing/validation happens before forwarding to IPC/handlers.
- **Forward compatibility behavior preserved**
  - Unknown JSON fields are ignored where required (envelope/payload tolerance).

**Typical deductions**
- Behavior differs from spec without deliberate spec update; broken hello gating; incorrect ID derivation/binding; replay loopholes; message forwarded before validation.

---

## 2) Security & Hardening (max 18)
Does the change preserve or improve the security posture under realistic threat models (malicious peers, hostile LAN, untrusted input, local IPC misuse)?

**Check:**
- **mTLS pinning and identity verification remain enforced**
  - Outbound SNI uses full typed agent ID (`ed25519.<hex>`).
  - Unknown inbound peers are rejected during TLS verification unless in expected peer table.
  - Expected-pubkey match is enforced when pinned.
- **Input validation + safe failure**
  - Malformed frames/JSON do not panic; invalid envelopes are rejected/dropped per spec.
  - No forwarding of malformed/unvalidated envelopes to IPC subscribers.
- **Resource/DoS controls preserved**
  - Bounded queues, bounded buffers, explicit drop/backpressure behavior.
  - Connection caps and handshake deadlines preserved.
- **Sensitive data protection**
  - No logging of private keys, raw key material, tokens, or sensitive payloads by default.
  - File/socket permissions remain strict (`0600` key/token/socket, `0700` dirs as applicable).
  - No "debug bypass" (accept-any-peer, disable verification) left enabled.

**Typical deductions**
- Weakening TLS verification/pinning; unbounded buffers; leaking secrets to logs; new attack surface without tests.

---

## 3) Testing Quality & Coverage (max 20)
Do tests *meaningfully verify behavior* and protect against regression?

**Check:**
- **Every behavior change has tests**
  - Unit tests cover new/changed functions and error paths.
  - Integration tests cover cross-module flows (IPC ↔ daemon ↔ transport, discovery ↔ pinning ↔ connect, reconnect loops).
- **Invariant-driven assertions**
  - Tests explicitly assert: hello gating, agent-id binding, initiator rule behavior, replay dedup, size limits.
- **Spec compliance tests updated when relevant**
  - If message kinds, schemas, or wire behavior change: update `axon/tests/spec_compliance.rs`.
- **Property testing where it pays off**
  - Parsing/validation/state machines: add or extend `proptest` coverage.
  - Commit any generated `proptest-regressions/`.
- **No flakiness**
  - Avoid "sleep and pray"; use signals/events; deterministic time control if available.

**Typical deductions**
- Missing regression tests; tests that only assert "no crash"; timing-dependent flakes; no spec compliance updates when needed.

---

## 4) Performance & Resource Efficiency (max 12)
Does the change preserve AXON's "fast and light" goals (<5MB RSS, negligible idle CPU), without premature micro-optimization?

**Check:**
- No obvious hot-path regressions (framing, envelope encode/decode, routing, replay cache).
- No blocking operations on async runtime (use `tokio::fs` / `spawn_blocking` when warranted).
- Resource limits remain bounded (buffers/queues, connection limits).
- Add/update benchmarks in `axon/benches/` when changing known hot paths (where practical).

**Typical deductions**
- Blocking I/O in async paths; unbounded allocations; noisy per-message logging; removing caps/timeouts.

---

## 5) Reliability, Error Handling & Robustness (max 10)
Does the daemon behave predictably under failures and malformed inputs?

**Check:**
- Errors are actionable and consistent (use established patterns; preserve context).
- Connection lifecycle remains robust (reconnect/backoff behavior not broken).
- Protocol-violation handling matches spec (drop vs respond with `error` depending on stream type).
- No panics in normal "bad input" conditions.

**Typical deductions**
- Panics on malformed input; swallowed errors; inconsistent drop/respond behavior; unreliable reconnect.

---

## 6) Maintainability & Code Health (max 12)
Will the code remain easy to change safely?

**Check:**
- Clear, idiomatic Rust; avoids "clever" patterns that reduce auditability.
- Consistent error types/contexts.
- Minimal duplication; shared logic centralized appropriately.
- Changes fit existing module boundaries (message/transport/daemon/ipc/discovery/etc.).
- **File size constraint honored**: Rust source files stay ≤ 500 lines; split modules when approaching.

**Typical deductions**
- Oversized files; ad-hoc JSON construction where typed structs prevent mistakes; broad refactors mixed with functional changes.

---

## 7) Build/CI Hygiene (max 8)
Does the change integrate cleanly with the repo workflow?

**Check:**
- `cargo fmt`, `cargo clippy -- -D warnings`, and full tests pass (or `make verify` in `axon/`).
- No new warnings; no ignored tests without explicit justification.
- No accidental debug artifacts; no dead code; no "TODO left behind" in critical paths.

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
