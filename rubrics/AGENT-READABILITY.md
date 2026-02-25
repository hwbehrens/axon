# AXON — AGENT-READABILITY Scoring Rubric

Status: Normative

Total: **100 points** across 6 categories.

This rubric evaluates the overall health of the repository from the perspective of AI/LLM/agent parsing and autonomous contribution. Focus on whether an agent can orient quickly, avoid context-window exhaustion, trust documentation accuracy, and safely make changes without guessing at process or structure.

Spec files are authoritative: `spec/SPEC.md`, `spec/WIRE_FORMAT.md`, `spec/MESSAGE_TYPES.md`, `spec/IPC.md`.

## Evaluation principles

Apply [`EVALUATION-PRINCIPLES.md`](EVALUATION-PRINCIPLES.md), especially material-impact scoring and shared severity calibration.

## Scoring method

Start each category at its maximum and deduct for findings.

---

## 1) Index Health and Navigation Aids (max 20)

Verify that an agent can orient and route tasks without broad search.

**Check:**

- A canonical deterministic read-order exists (in `AGENTS.md`) and all referenced files are present.
- A machine-readable agent map (`docs/agent-index.json`) is valid JSON with subsystems, code roots, test roots, key files, and validation commands.
- Every top-level runtime subsystem has an entry in the agent map with clear ownership boundaries.
- Per-subsystem validation commands are provided and runnable.
- Common-task routing entries map task types to primary files and co-change targets.

**Typical deductions**

- Missing or competing read-orders; invalid or unparseable agent index; subsystems with no agent-map entry; common tasks without co-change guidance.

---

## 2) Context-Budget Discipline (max 20)

Verify that an agent can understand and modify code without exhausting its context window.

**Check:**

- Rust source files respect the stated line limit (≤ 500 lines); files approaching the limit are proactively split.
- Key logic lives in shallow module trees; average path depth is reasonable and predictable.
- Boilerplate is minimized; repetitive patterns are abstracted or isolated from core logic.
- Large artifacts (binaries, generated code, data blobs) are segregated under clearly non-normative or ignored directories.
- Test entrypoints and scenario registries are discoverable without loading entire test internals.

**Typical deductions**

- Source files exceeding line limits; deeply nested directories without maps; excessive boilerplate in protocol-critical logic; large data files in source directories.

---

## 3) Documentation Drift and Normative Coherence (max 20)

Verify that docs, specs, and code tell a consistent story.

**Check:**

- An explicit authority hierarchy exists (`spec/*` > `AGENTS.md` / `CONTRIBUTING.md` > `README.md` > code comments) with escalation behavior defined for conflicts.
- Every normative document has an explicit status (`Normative`, `Draft`, `Archived`) and the status matrix in `AGENTS.md` matches the headers inside each file.
- `README.md` Configuration Reference tables track all configurable settings and internal constants.
- `docs/decision-log.md` and `docs/open-questions.md` are actively maintained; old entries are resolved or linked to current status.
- Spec clauses are traceable to code (via module docs, test names, or rubric trace matrix).

**Typical deductions**

- Conflicting authority with no escalation rule; status matrix disagreeing with document headers; constants in code missing from README; stale decision log or open questions; no spec-to-rubric traceability.

---

## 4) Cross-Reference and Path Integrity (max 15)

Verify that internal links, indexes, and commands actually resolve.

**Check:**

- Markdown links within normative docs (specs, rubrics, AGENTS.md) resolve to existing files and anchors.
- All paths in `docs/agent-index.json` (`key_files`, `code_roots`, `test_roots`) point to files or directories that exist.
- Subsystem boundaries in the agent map match actual module boundaries.
- Validation commands listed in the agent index succeed on a clean checkout with documented prerequisites.

**Typical deductions**

- Broken links in normative documents; stale paths in agent index; subsystem map entries contradicting module layout; validation commands that fail.

---

## 5) Tidiness and Noise Control (max 15)

Verify that irrelevant or stale content does not waste agent attention.

**Check:**

- No orphaned modules or dead code that appears active; deprecated code is archived or removed.
- `cargo clippy -D warnings` is enforced; unused imports and warnings are uncommon.
- TODOs and FIXMEs have owners, dates, or issue IDs; open questions are centralized in `docs/open-questions.md` rather than scattered in code.
- Stale or abandoned directories are documented as non-normative or removed.
- No duplicated guidance across docs/specs; where summaries exist, they are cross-linked and consistent.

**Typical deductions**

- Orphaned code that agents may mistake for active logic; persistent lint warnings; large numbers of untracked TODOs; duplicated but inconsistent guidance.

---

## 6) Agent-Operability Guardrails (max 10)

Verify that an agent can safely make changes without guessing at process.

**Check:**

- `AGENTS.md` files are present at root and at each major subsystem scope listed in the nested-AGENTS index.
- Hard constraints (file line limits, test separation, constants co-change) are explicitly stated and enforceable via CI or Makefile.
- "What to update when you change X" rules are enumerated for module renames, constant changes, and spec edits.
- Common-task golden paths (primary files + tests to run) are maintained in the agent index.

**Typical deductions**

- Missing nested AGENTS files for subsystems that declare them; hard constraints that are routinely violated; missing co-change rules causing partial updates; common-task entries that are outdated.

---

## Score Sheet

| Category | Max | Score |
|---|---:|---:|
| 1) Index Health and Navigation Aids | 20 | /20 |
| 2) Context-Budget Discipline | 20 | /20 |
| 3) Documentation Drift and Normative Coherence | 20 | /20 |
| 4) Cross-Reference and Path Integrity | 15 | /15 |
| 5) Tidiness and Noise Control | 15 | /15 |
| 6) Agent-Operability Guardrails | 10 | /10 |
| **Total** | **100** | **/100** |
