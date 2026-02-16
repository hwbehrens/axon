# AXON — ALIGNMENT Scoring Rubric (Project Alignment / LLM-First Fit)
Total: **100 points** across 6 categories.

This rubric evaluates whether a change is aligned with AXON's goals and philosophy:
- **LLM-first protocol** (structured, learnable, low context overhead)
- **Spec is authoritative** (implementation must follow `spec/`)
- **Lightweight daemon** (fast, minimal dependencies, bounded resources)
- **Simplicity-first** (avoid over-engineering; changes should be easy for agents to navigate)

## Scoring guidance
- Start each category at max and deduct.
- Alignment is partly holistic; deductions must cite concrete signals (diff size, new abstractions, added deps, spec drift, naming, file size, etc.).

---

## 1) Spec-First & Interop Mindset (max 20)
AXON is a protocol project. Other implementations should be possible **without reading Rust source**.

**Check:**
- Behavior matches the normative spec (`spec/SPEC.md`, `spec/WIRE_FORMAT.md`, `spec/MESSAGE_TYPES.md`, `spec/IPC.md`).
- If behavior changes affect interoperability, the PR updates the spec and spec compliance tests in the same change.
- No "implementation-only" protocol behavior (undocumented special cases, magic constants, hidden negotiation rules).
- Preserves forward compatibility principles (unknown fields tolerated where required; stable envelope shape).

**Typical deductions**
- Silent protocol behavior drift; "it works in Rust" but not documented; changes that would break other-language implementations without spec updates.

---

## 2) Simplicity / YAGNI / Minimal Dependencies (max 18)
AXON optimizes for maintainability and agent productivity, not maximal feature surface.

**Check:**
- Minimal, incremental change: smallest diff that solves the problem.
- No new dependencies unless clearly justified and consistent with repo conventions.
- Avoids adding new layers/traits/config knobs unless there is a demonstrated need.
- Avoids broad refactors mixed into functional changes.
- Keeps code paths straightforward and auditable (especially in transport/security).

**Typical deductions**
- New abstraction without clear payoff; adding dependencies for convenience; refactors that increase conceptual load for LLMs.

---

## 3) LLM-First Navigability & Learnability (max 22)
The repository is built "by and for LLM agents." Changes should make the codebase easier for agents to read, reason about, and modify.

**Check:**
- **Naming is semantic and explicit**
  - Fields/vars use full meaning (`question`, `report_back`, `buffer_ttl_secs`), not abbreviations.
- **Structure supports single-pass reading**
  - Rust files remain ≤ 500 lines; modules split cleanly.
  - Logic placed where `AGENTS.md` / `CONTRIBUTING.md` module map says it belongs.
- **Patterns are consistent**
  - Request/response flows follow established bidi stream pattern; fire-and-forget uses uni streams; no "special cases."
- **Errors are instructive (where user/agent-facing)**
  - `error` payloads guide next action (per message type guidance), not just "permission denied."

**Typical deductions**
- Hard-to-follow control flow; scattered logic; "clever" metaprogramming; non-semantic names; overlong files; inconsistent protocol patterns.

---

## 4) Architectural Coherence & Load-Bearing Invariants (max 18)
AXON has explicit invariants (hello gating, pinning, initiator rule, replay protection, bounded buffers).

**Check:**
- Invariants from `AGENTS.md` / `CONTRIBUTING.md` are preserved.
- Any change touching these invariants is:
  - encoded in tests (unit/integration/spec compliance)
  - reflected in spec if externally visible
  - implemented in the correct layer (TLS checks in TLS verifier, hello gating in handshake layer, etc.)
- The daemon remains a lightweight transport + router (no store-and-forward semantics beyond the **local IPC receive buffer**).

**Typical deductions**
- Invariant changes without explicit spec/tests; shifting responsibilities across layers; adding stateful "features" that contradict the daemon's role.

---

## 5) Efficiency & Context-Budget Awareness (max 12)
AXON is "context-budget-aware": avoid token waste and unnecessary overhead.

**Check:**
- Wire messages remain compact JSON (no pretty printing on the wire).
- Logging/telemetry does not spam (especially under adversarial input); avoids verbose per-message logs by default.
- IPC and network payload schemas stay structured-first (machine-parseable), avoid natural-language ceremony.
- Avoids chatty protocols: prefers single rich exchanges over multi-round-trips unless required.

**Typical deductions**
- New verbose logs; message bloat; "human chat" strings in protocol fields; extra round-trips without need.

---

## 6) Operational Philosophy Fit (max 10)
AXON is intended to run indefinitely, low resource, predictable behavior.

**Check:**
- Maintains bounded resource usage and explicit caps (connections, buffers, queue depths).
- Maintains backward compatibility rules where required (IPC v1 behavior unless hardened mode configured).
- Keeps configuration surface disciplined:
  - if adding/changing a configurable setting, it belongs in `Config` and README config tables are updated (see documentation rubric too).

**Typical deductions**
- Unbounded growth; noisy behavior under attack; new config knobs without documentation or clear need.

---

## Score Sheet
| Category | Max | Score |
|---|---:|---:|
| 1) Spec-First & Interop Mindset | 20 | /20 |
| 2) Simplicity / YAGNI / Minimal Dependencies | 18 | /18 |
| 3) LLM-First Navigability & Learnability | 22 | /22 |
| 4) Architectural Coherence & Invariants | 18 | /18 |
| 5) Efficiency & Context-Budget Awareness | 12 | /12 |
| 6) Operational Philosophy Fit | 10 | /10 |
| **Total** | **100** | **/100** |
