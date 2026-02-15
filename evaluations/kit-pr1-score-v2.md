# PR #1 Evaluation v2: Rescore after test additions

**Reviewer:** Judge (Kit self-review, round 2)  
**Date:** 2026-02-15  
**Previous Score:** 76/100  

---

## What Changed Since v1

Two new commits added on top of the original:
- `58b7452 test: add connection handler invariant tests` — 4 integration tests in `violations.rs`
- `e0de5f9 test: add spec compliance tests for error codes` — 2 spec compliance tests in `wire_format.rs`

I'll re-evaluate each category, noting what improved and what didn't.

---

## 1. Security & Hardening (14/18)

**Improved from 12 → 14.** The new tests partially address the "no security tests for refactored code" concern. `connection_all_bidi_kinds_get_correct_response` exercises the full post-hello pipeline through the wire, which implicitly validates that gating, routing, and response generation survive the refactor. `connection_uni_notify_delivered` confirms uni stream delivery works end-to-end.

**Remaining deductions:**
- **-3: IPC auth is still a TODO, not a fix.** Unchanged from v1. The comment documents the attack surface but doesn't mitigate it.
- **-1: No test for hello rejection / failed auth path.** The tests cover happy-path hello (implicitly via `send`) but don't explicitly test that a peer with a *wrong* certificate or unexpected identity gets rejected post-refactor. The existing test suite has some coverage here, but the refactored `handle_hello` function — which now owns this gate independently — doesn't have a dedicated regression test.

## 2. Test Quality & Coverage (12/18)

**Improved from 6 → 12.** This was the weakest category and saw the most improvement. But the improvement is uneven — some tests are strong, others have real problems.

**What's good:**
- `connection_all_bidi_kinds_get_correct_response` is the standout. It exercises 5 request/response kind pairs through the full wire path, validating `handle_bidi_stream` → `handle_authenticated_bidi` → `auto_response`. It checks both response kind and `ref_id` linkage. This is a proper invariant-driven integration test.
- `connection_uni_notify_delivered` exercises `handle_uni_stream` end-to-end. Clean assertions on message ID and sender identity.
- `invalid_envelope_error_code_in_spec` and `all_spec_error_codes_serialize_correctly` directly address the v1 complaint about missing spec compliance tests. The latter is particularly useful — it catches future renames or removals of any error code.

**What's weak or broken:**
- **-2: `connection_bidi_unknown_kind_returns_error` doesn't test what it claims.** The test constructs a JSON envelope with `"totally_made_up_kind"`, deserializes it, and asserts `parsed.kind == MessageKind::Unknown`. Then it *stops*. It never sends this envelope over the wire. It never receives the `error(unknown_kind)` response. This is a serde unit test masquerading as a connection handler test. The function name and docstring ("Exercises: handle_authenticated_bidi (unknown kind branch)") are misleading — it exercises `serde_json::from_slice`, not the handler. The unknown-kind error response path in `handle_authenticated_bidi` remains untested.
- **-2: `connection_replay_protection_drops_duplicates` doesn't test replay.** The test sends *one* query and asserts it gets a response. It never sends a duplicate with the same message ID. The test name says "drops duplicates" but tests "first message works." Furthermore, it constructs `transport_b` with `None` for the replay check callback, meaning replay protection isn't even enabled on the receiver. This test proves nothing about replay — it's a basic connectivity check with a misleading name.
- **-2: No unit tests for extracted helpers.** The v1 feedback specifically noted that `handle_uni_stream`, `handle_bidi_stream`, `handle_hello`, `handle_authenticated_bidi`, and `send_response` are now testable in isolation. All new tests are integration-level (full transport setup). The opportunity to write focused unit tests against the clean helper APIs was still not taken. Integration tests are valuable but coarser-grained — a bug in `handle_hello`'s error path might not surface if the transport layer masks it.

## 3. Performance & Efficiency (14/14)

**Unchanged.** No new performance concerns. The test code doesn't impact production paths.

## 4. Maintainability & Code Quality (13/14)

**Unchanged.** The new test commits are cleanly separated from the original refactor commit (good — addresses the "mixed concerns" issue partially). Test code is readable and follows existing patterns.

**Remaining deduction:**
- **-1: Original refactor commit is still a grab-bag.** The refactor, spec fixes, and CONTRIBUTING.md changes are still in one commit. The new test commits are properly separated, which shows the author understands commit hygiene — making the original commit's structure more conspicuous, not less.

## 5. Operational Maturity (9/10)

**Unchanged.** Tests don't impact operational behavior.

**Remaining deduction:**
- **-1: No `#[instrument]` spans on refactored helpers.** Minor, unchanged from v1.

## 6. Adversarial Robustness (9/10)

**Improved from 8 → 9.** The `connection_all_bidi_kinds_get_correct_response` test, while not adversarial per se, exercises every request kind through the handler pipeline — which means protocol violations in kind routing would surface. The spec compliance tests for error codes provide a regression net against error code schema drift.

**Remaining deduction:**
- **-1: Still no adversarial-specific tests for the refactored surface.** No tests for: malformed JSON on bidi streams (does `handle_bidi_stream` return the right error?), oversized envelopes hitting the new helpers, pre-hello uni messages from a mismatched `from` field. The `connection_bidi_unknown_kind_returns_error` test *could* have been this, but as noted above, it doesn't actually send anything over the wire.

## 7. Contribution Hygiene (9/10)

**Improved from 8 → 9.** Tests are now included. The new commits are properly scoped and clearly messaged (`test: add connection handler invariant tests`, `test: add spec compliance tests for error codes`). The PR now contains tests for the behavioral changes.

**Remaining deduction:**
- **-1: Two of the four integration tests have correctness issues** (misleading names/incomplete assertions, as detailed in §2). Shipping tests that don't test what they claim is arguably worse than shipping no tests — it creates false confidence. A reviewer seeing `connection_replay_protection_drops_duplicates` in the test suite would reasonably believe replay protection is tested. It isn't.

## 8. Interop & Spec Drift Control (6/6)

**Unchanged, still perfect.** The new `all_spec_error_codes_serialize_correctly` test strengthens this category further — now *all* error codes have a serialization regression test, not just `invalid_envelope`. But the score was already 6/6, so it can't go higher.

---

## Summary Table

| Category | Max | v1 | v2 | Δ |
|---|---|---|---|---|
| 1. Security & Hardening | 18 | 12 | 14 | +2 |
| 2. Test Quality & Coverage | 18 | 6 | 12 | +6 |
| 3. Performance & Efficiency | 14 | 14 | 14 | — |
| 4. Maintainability & Code Quality | 14 | 13 | 13 | — |
| 5. Operational Maturity | 10 | 9 | 9 | — |
| 6. Adversarial Robustness | 10 | 8 | 9 | +1 |
| 7. Contribution Hygiene | 10 | 8 | 9 | +1 |
| 8. Interop & Spec Drift Control | 6 | 6 | 6 | — |
| **Total** | **100** | **76** | **86** | **+10** |

---

## Verdict

**Score: 86/100** (up from 76)

The test additions are a meaningful improvement. Two of the six new tests are genuinely strong (`connection_all_bidi_kinds_get_correct_response` is excellent, and `all_spec_error_codes_serialize_correctly` is exactly the kind of regression test the rubric asks for). Two others are solid if unremarkable (`connection_uni_notify_delivered`, `invalid_envelope_error_code_in_spec`).

But two tests are broken in spirit: `connection_bidi_unknown_kind_returns_error` never sends anything over the wire, and `connection_replay_protection_drops_duplicates` never sends a duplicate. These aren't just "incomplete" — they're *misleading*. A test named "drops duplicates" that never creates a duplicate is the kind of thing that makes a reviewer trust the suite less, not more. If these two tests did what their names promise, this would be a 90+ score.

## What Would Get This to 90+

1. **Fix `connection_bidi_unknown_kind_returns_error`**: Actually send the unknown-kind envelope through the transport. Assert the response is `error` with code `unknown_kind`.
2. **Fix `connection_replay_protection_drops_duplicates`**: Enable replay checking on the receiver. Send the same envelope twice (same UUID). Assert the first gets a response and the second is dropped (no delivery to inbound subscribers, or timeout on second send).
3. **Add one adversarial test**: Malformed JSON on a bidi stream → verify connection survives and returns an appropriate error (or drops gracefully).
4. **Add a hello rejection test**: Wrong peer identity → verify `handle_hello` returns `None` and the connection closes.
