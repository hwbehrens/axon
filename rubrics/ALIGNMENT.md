# AXON — ALIGNMENT Scoring Rubric (Project Alignment / LLM-First Fit)

Status: Normative

Total: **100 points** across 6 categories.

This rubric evaluates whether a change is aligned with AXON's goals and philosophy:
- **LLM-first protocol** (structured, learnable, low context overhead)
- **Spec is authoritative** (implementation must follow `spec/`)
- **Lightweight daemon** (fast, minimal dependencies, bounded resources)
- **Simplicity-first** (avoid over-engineering; changes should be easy for agents to navigate)

Spec files are authoritative: `spec/SPEC.md`, `spec/WIRE_FORMAT.md`, `spec/MESSAGE_TYPES.md`, `spec/IPC.md`.

## Evaluation principles

Apply [`EVALUATION-PRINCIPLES.md`](EVALUATION-PRINCIPLES.md), especially material-impact scoring and shared severity calibration.

## Scoring method
- Start each category at its maximum and deduct for findings.
- Alignment is partly holistic; deductions must cite concrete signals (diff size, new abstractions, added deps, spec drift, naming, file size, etc.).

---

## 1) Spec-First & Interop Mindset (max 20)
AXON is a protocol project. Other implementations should be possible **without reading Rust source**.

**Check:**
- Behavior matches the normative spec (`spec/SPEC.md`, `spec/WIRE_FORMAT.md`, `spec/MESSAGE_TYPES.md`, `spec/IPC.md`).
- If behavior changes affect interoperability, the PR updates the spec and spec-compliance tests in the same change.
- No implementation-only protocol behavior (undocumented special cases, magic constants, hidden negotiation rules).
- Preserves forward-compatibility principles (unknown fields tolerated where required; stable envelope shape).

**Typical deductions**
- Silent protocol behavior drift; behavior that only works by reading Rust internals; spec updates missing for interop-relevant changes.

---

## 2) Simplicity / YAGNI / Minimal Dependencies (max 18)
AXON optimizes for maintainability and agent productivity, not maximal feature surface.

**Check:**
- Minimal, incremental change: smallest diff that solves the problem.
- No new dependencies unless clearly justified and consistent with repo conventions.
- Avoids adding new layers/traits/config knobs unless there is a demonstrated need.
- Avoids broad refactors mixed into functional changes.
- Keeps code paths straightforward and auditable, especially in transport and security.

**Typical deductions**
- New abstraction without clear payoff; dependencies added for convenience; refactors that increase conceptual load for LLMs.

---

## 3) LLM-First Navigability & Learnability (max 22)
The repository is built by and for LLM agents. Changes should make the codebase easier for agents to read, reason about, and modify.

**Check:**
- **Naming is semantic and explicit**
  - Fields/vars use full meaning (`question`, `report_back`, `buffer_ttl_secs`), not abbreviations.
- **Structure supports single-pass reading**
  - Rust files remain <= 500 lines; modules split cleanly.
  - Logic stays where `AGENTS.md` / `CONTRIBUTING.md` says it belongs.
- **Patterns are consistent**
  - Stream usage and message flow follow spec-defined patterns; no ad hoc special paths.
- **Errors are instructive where user/agent-facing**
  - `error` payloads guide next action per protocol guidance.

**Typical deductions**
- Hard-to-follow control flow; scattered logic; non-semantic names; oversized files; inconsistent protocol patterns.

---

## 4) Architectural Coherence & Load-Bearing Invariants (max 18)
AXON has explicit invariants that must stay coherent across code, tests, and docs.

**Check:**
- Invariants from `AGENTS.md`, `CONTRIBUTING.md`, and the `spec/` files are preserved.
- Any change touching invariants is:
  - encoded in tests (unit/integration/spec-compliance)
  - reflected in spec when externally visible
  - implemented in the correct layer with clear responsibility boundaries
- The daemon remains a lightweight transport + router unless the spec intentionally expands its role.

**Typical deductions**
- Invariant changes without matching spec/tests; responsibility drift across layers; stateful features that conflict with daemon scope.

---

## 5) Efficiency & Context-Budget Awareness (max 12)
AXON is context-budget-aware: avoid token waste and unnecessary overhead.

**Check:**
- Wire messages remain compact JSON (no pretty printing on the wire).
- Logging/telemetry avoids spam, especially under adversarial input.
- IPC and network payloads remain structured and machine-parseable per spec.
- Avoids chatty protocols: prefer single rich exchanges over extra round-trips unless required.

**Typical deductions**
- Verbose logs by default; protocol message bloat; extra round-trips without clear need.

---

## 6) Operational Philosophy Fit (max 10)
AXON is intended to run indefinitely with low resource overhead and predictable behavior.

**Check:**
- Maintains bounded resource usage and explicit caps (connections, buffers, queue depths).
- Maintains compatibility and migration rules defined in current specs and docs.
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
