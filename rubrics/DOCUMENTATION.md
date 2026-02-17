# AXON — DOCUMENTATION Scoring Rubric (Specs, README, Guides, Self-Documentation)
Total: **100 points** across 6 categories.

This rubric evaluates whether documentation stays accurate, authoritative, and LLM-usable.
AXON's rule: **the spec is authoritative** (`spec/` wins over implementation).

Spec files are authoritative: `spec/SPEC.md`, `spec/WIRE_FORMAT.md`, `spec/MESSAGE_TYPES.md`, `spec/IPC.md`.

## Evaluation principles

You are an impartial, rigorous technical reviewer. Follow these principles:

- **Fair and evidence-based.** Every deduction must cite a concrete, verifiable signal in the diff, code, spec, or documentation; never penalize on vague intuition. Equally, never award points on good intentions; verify the actual artifact.
- **First-principles thinking.** Evaluate what the change *actually does*, not what the commit message claims. Read the code; read the spec; check that they agree. If they disagree, that is a finding.
- **100 means flawless.** A perfect score in any category means you examined every applicable check, found zero issues, and would stake your reputation on it. Do not round up. If in doubt, deduct; the author can rebut.
- **This is not a rubber stamp.** Assume the change has defects until proven otherwise. Actively look for spec drift, stale references, broken links, outdated examples, missing config-table entries, guidance that contradicts implementation, and undocumented behavior changes.
- **Thorough, not cursory.** Read the actual files, not just the diff summary. Follow documentation links and verify they resolve. Check that constants in spec match constants in code. Confirm CLI help text matches current behavior.
- **Deductions are cumulative and specific.** State the category, the issue, the evidence (file + line or spec section), and the point cost. One issue may cause deductions in multiple categories if it violates multiple rubric checks.
- **Proportional severity.** A spec that contradicts implementation on an interop-critical rule warrants a larger deduction than a minor formatting inconsistency. Use judgment, but always explain the reasoning.
- **Substance over preference.** Focus on issues that affect correctness, interoperability, or discoverability, not stylistic preferences about prose, formatting, or document organization. A finding is substantive if ignoring it could cause a reimplementation to produce incompatible behavior, leave a developer unable to find critical information, or create a contradiction between authoritative documents. A finding is a nit if it reflects a reviewer preference that reasonable technical writers would disagree on. Deduct for substantive issues; do not deduct for nits.

## Scoring method
- Start each category at its maximum and deduct for findings.
- Documentation includes: `spec/*.md`, `README.md`, `AGENTS.md`, `CONTRIBUTING.md`, CLI help text, and code-level docs where appropriate.
- "No doc changes needed" is valid only when the change is truly internal and non-user-visible.

---

## 1) Spec Accuracy & Interop Documentation (max 30)
Do the normative specs remain correct and sufficient to implement AXON in another language without reading Rust?

**Check:**
- If any externally visible behavior changes (wire format, message kinds, IPC protocol, limits, CLI surface):
  - update the relevant spec files: `spec/SPEC.md`, `spec/WIRE_FORMAT.md`, `spec/MESSAGE_TYPES.md`, `spec/IPC.md`
  - update interoperability checklists and constants tables when impacted
  - include executable conformance checks when practical (avoid relying only on prose review)
- Spec language uses RFC2119 keywords appropriately for normative requirements.
- Spec and implementation agree on critical constants and invariants; consult the authoritative `spec/` files for the current list.
- Forward-compatibility expectations remain documented where required.

**Typical deductions**
- Implementation-only behavior; spec drift; changed constants without spec updates; incomplete interop guidance.

---

## 2) README & Configuration Reference (max 20)
Is `README.md` accurate for users and agents?

**Check:**
- Quickstart steps still work (build/run/send examples).
- Message type summary remains correct (kind ↔ stream mapping / purpose) and aligned with `spec/MESSAGE_TYPES.md`.
- **Configuration Reference tables are updated** when config keys or internal constants change:
  - `config.toml` keys table
  - internal constants table (e.g., `MAX_MESSAGE_SIZE`, timeouts, caps)
- Documentation links remain valid and well-scoped.

**Typical deductions**
- Added config fields not reflected; outdated defaults; examples that no longer match CLI behavior.

---

## 3) Agent/Contributor Guidance (AGENTS / CONTRIBUTING) (max 15)
AXON is built for LLM agents; repo guidance is part of the product.

**Check:**
- If module map changes (new files, moved responsibilities), update:
  - `AGENTS.md` repository layout/module map
  - `CONTRIBUTING.md` change-to-file guidance
- If testing requirements or workflows change, update `CONTRIBUTING.md` and/or `AGENTS.md` verification commands.
- Invariants list stays correct and prominent, with references to authoritative spec sections.

**Typical deductions**
- New modules with no map updates; guidance drifting away from reality; missing invariant updates.

---

## 4) Code-Level Documentation & Self-Documenting Code (max 15)
Are comments and docstrings appropriate and helpful, without adding noise?

**Check:**
- Public APIs / key types have minimal, clear rustdoc where it reduces ambiguity.
- Complex logic has brief why-comments (not what-comments), especially in security-critical and protocol-critical paths.
- Code remains self-documenting: semantic names, clear types, minimal ad-hoc JSON.
- Avoid long prose in code when the spec is the right place (spec remains authoritative).

**Typical deductions**
- Missing explanation for tricky invariants; comment spam; duplicating spec text in code instead of updating spec.

---

## 5) Examples, CLI Help, and Learnability Aids (max 10)
Can a new LLM agent learn AXON quickly from the repo?

**Check:**
- CLI help and subcommand help remain accurate and self-describing (full words, not cryptic abbreviations).
- Example interactions (e.g., `axon examples`) remain accurate if touched.
- Examples/help claims are validated against executable behavior (`axon --help`, targeted contract tests), not reviewer assumptions.
- Error messages intended for agents are instructive and suggest next actions (especially protocol/IPC errors).

**Typical deductions**
- CLI help drift; broken/incorrect examples; unhelpful errors.

---

## 6) Change Communication & Reviewability (max 10)
Does the change explain itself for reviewers and future maintainers?

**Check:**
- PR/commit description (or equivalent) clearly states:
  - what changed, why, and user-visible impact
  - whether specs were updated (and why not, if not)
  - upgrade or compatibility considerations when applicable
- If behavior changes might surprise users/agents, the docs call it out.

**Typical deductions**
- No explanation for user-visible changes; missing compatibility notes; reviewers must infer intent from code only.

---

## Score Sheet
| Category | Max | Score |
|---|---:|---:|
| 1) Spec Accuracy & Interop Documentation | 30 | /30 |
| 2) README & Configuration Reference | 20 | /20 |
| 3) Agent/Contributor Guidance | 15 | /15 |
| 4) Code-Level Documentation & Self-Documenting Code | 15 | /15 |
| 5) Examples, CLI Help, Learnability | 10 | /10 |
| 6) Change Communication & Reviewability | 10 | /10 |
| **Total** | **100** | **/100** |
