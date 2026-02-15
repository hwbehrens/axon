# PR #1 Evaluation v3: Rescore after test rewrites

**Reviewer:** Judge (Kit self-review, round 3)  
**Date:** 2026-02-15  
**Previous Scores:** 76/100 → 86/100  

---

## What Changed Since v2

One new commit: `498f6d0 test: fix misleading tests (Judge v2 feedback)`. The two tests flagged as broken in v2 — `connection_bidi_unknown_kind_returns_error` and `connection_replay_protection_drops_duplicates` — were rewritten. Let me be skeptical.

---

## Deep Dive: The Two Rewritten Tests

### `connection_bidi_unknown_kind_returns_error`

**v2 problem:** Only tested serde deserialization. Never sent anything over the wire.

**v3 implementation:** Establishes a real QUIC connection (hello completes via `ensure_connection`), then manually crafts JSON with `"kind": "totally_made_up_kind"`, writes raw bytes to a bidi stream via `conn.open_bi()`, reads the response, and asserts `response.kind == MessageKind::Error` with `payload["code"] == "unknown_kind"`.

**Verdict: Fixed. Genuinely exercises the wire path.** The test bypasses the typed send API (which would reject unknown kinds at compile time) and instead writes raw bytes directly to a QUIC bidi stream. The response is read from the same stream. This exercises `handle_bidi_stream` → `handle_authenticated_bidi` → unknown kind branch → `send_response`, which is exactly what the test name claims.

One minor note: the test uses `recv.read_to_end(65536)` rather than `read_framed`, meaning it relies on QUIC's native FIN-delimited stream semantics. This is fine — it's actually more correct than using the internal framing helper, since it validates that the response is properly finished.

### `connection_replay_protection_drops_duplicate_uni`

**v2 problem:** Test was named `connection_replay_protection_drops_duplicates` but only sent one message, with replay checking disabled (`None`).

**v3 implementation:** Complete rewrite. Creates a `ReplayCheckFn` backed by a `HashSet<Uuid>` that returns `true` (is replay) when an ID has been seen. Binds `transport_b` via `bind_cancellable` with this replay checker. Sends the *same envelope bytes* twice over *separate uni streams* via `conn.open_uni()`. Drains the inbound subscriber and asserts `notify_count == 1`.

**Verdict: Fixed. Properly tests replay dedup.** Key things I verified:
- Replay checker is actually wired in via `bind_cancellable` (not `bind` which passes `None`).
- Same bytes = same UUID, so the second send triggers the replay check.
- Uses raw `open_uni()` + `write_all` to bypass any client-side dedup.
- Drains with a short timeout after a sleep, counting only notifies matching the specific UUID.
- The `sleep(500ms)` before draining is a "sleep and pray" pattern — the rubric dislikes this. However, for uni streams (fire-and-forget, no response to await), there's no clean signal to wait on. Acceptable pragmatism, minor smell.

---

## Category Scores

### 1. Security & Hardening (15/18)

**Improved from 14 → 15.** The unknown-kind test now exercises an adversarial-adjacent path through the wire (sending a kind the receiver doesn't recognize). The replay test validates that the dedup gate actually drops duplicates.

**Remaining deductions:**
- **-3: IPC auth is still a TODO.** Unchanged. A comment is not a mitigation.

### 2. Test Quality & Coverage (15/18)

**Improved from 12 → 15.** Both previously-broken tests now do what their names claim. The test suite for this PR is now genuinely strong:

- `connection_bidi_unknown_kind_returns_error` — wire-level, bypasses typed API, asserts error code ✓
- `connection_replay_protection_drops_duplicate_uni` — replay checker wired in, sends duplicate, asserts single delivery ✓  
- `connection_all_bidi_kinds_get_correct_response` — comprehensive, 5 kind pairs, checks ref_id ✓
- `connection_uni_notify_delivered` — clean end-to-end ✓
- `invalid_envelope_error_code_in_spec` and `all_spec_error_codes_serialize_correctly` — spec regression nets ✓

**Remaining deductions:**
- **-1: No unit tests for extracted helpers.** All tests are integration-level. The refactored helpers (`handle_hello`, `handle_authenticated_bidi`, etc.) could be tested in isolation for faster, more targeted feedback. This is a style preference but the rubric does ask for unit tests.
- **-1: No hello rejection test.** The `handle_hello` failure path (wrong certificate, unexpected peer) still has no dedicated regression test post-refactor.
- **-1: Sleep-based timing in replay test.** The `sleep(500ms)` + short-timeout drain pattern is fragile under CI load. Not a dealbreaker, but the rubric flags "sleep and pray."

### 3. Performance & Efficiency (14/14)

**Unchanged.** No concerns.

### 4. Maintainability & Code Quality (13/14)

**Unchanged.**

- **-1: Original refactor commit is still a grab-bag** (refactor + spec fixes + CONTRIBUTING.md in one commit).

### 5. Operational Maturity (9/10)

**Unchanged.**

- **-1: No `#[instrument]` spans on refactored helpers.** Minor.

### 6. Adversarial Robustness (10/10)

**Improved from 9 → 10.** The unknown-kind-over-wire test is genuinely adversarial — it sends a fabricated message kind that can't be constructed through normal APIs. The replay test validates dedup under intentional duplication. Together with the existing adversarial tests in the suite, this category is now fully covered for the scope of this PR.

### 7. Contribution Hygiene (9/10)

**Unchanged from v2.**

- **-1: Original refactor commit structure.** The fix commits are well-scoped, but the first commit remains a grab-bag.

### 8. Interop & Spec Drift Control (6/6)

**Unchanged, still perfect.**

---

## Summary Table

| Category | Max | v1 | v2 | v3 | Δ(v2→v3) |
|---|---|---|---|---|---|
| 1. Security & Hardening | 18 | 12 | 14 | 15 | +1 |
| 2. Test Quality & Coverage | 18 | 6 | 12 | 15 | +3 |
| 3. Performance & Efficiency | 14 | 14 | 14 | 14 | — |
| 4. Maintainability & Code Quality | 14 | 13 | 13 | 13 | — |
| 5. Operational Maturity | 10 | 9 | 9 | 9 | — |
| 6. Adversarial Robustness | 10 | 8 | 9 | 10 | +1 |
| 7. Contribution Hygiene | 10 | 8 | 9 | 9 | — |
| 8. Interop & Spec Drift Control | 6 | 6 | 6 | 6 | — |
| **Total** | **100** | **76** | **86** | **91** | **+5** |

---

## Verdict

**Score: 91/100** (up from 86)

The two rewritten tests are legitimate. `connection_bidi_unknown_kind_returns_error` now sends fabricated wire bytes through a real QUIC bidi stream and validates the error response — it's a proper adversarial integration test. `connection_replay_protection_drops_duplicate_uni` wires in a real replay checker, sends the same envelope twice, and asserts single delivery — it tests exactly what the name promises.

The remaining 9 points are:
- 3 for the IPC auth TODO (a comment is not a fix)
- 3 for test gaps (no unit tests for helpers, no hello rejection test, sleep-based timing)
- 1 for commit hygiene (grab-bag first commit)
- 1 for missing `#[instrument]` spans
- 1 for maintainability (commit structure)

This is a solid PR. The refactor is clean, the tests are now honest, and the spec fixes are correct.

## What Would Get This to 95+

1. **Add a hello rejection test** — wrong peer cert → verify `handle_hello` returns error and connection closes.
2. **Replace sleep with a deterministic signal** in the replay test (e.g., a counter/latch on the replay checker callback).
3. **Split the original commit** into refactor, spec fix, and docs.
4. **Add `#[instrument]` spans** to the new helper functions.
