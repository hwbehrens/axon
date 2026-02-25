# LLM-Friendliness Remediation Plan

Status: Draft

## Background

A structural comparison with `plasma-bft` identified LLM-optimization patterns that axon could adopt. A Socratic exchange between two LLM agents (one representing each project) refined the recommendations by stress-testing assumptions about context efficiency, auto-loading behavior, and failure mode severity.

### Key insights from the cross-project analysis

1. **"Dense" and "context-efficient" are not the same thing.** Fewer files ≠ less wasted context. A 25-line auto-loaded `transport/AGENTS.md` is cheaper than scanning 100 lines of root AGENTS.md for the 20 relevant lines. The right metric is relevant tokens per task, not file count.

2. **Auto-loading eliminates the discovery overhead** that was the main argument against hierarchical guidance. Nested AGENTS.md files are contextually injected when an agent touches files in that directory — the agent pays no navigation cost.

3. **Partial adoption of nested AGENTS.md is worse than all-or-nothing.** If some modules have AGENTS.md and others don't, the agent must reason about whether guidance exists per-directory. Consistency wins.

4. **A decision log's value is search, not sequential reading.** `grep "identity" docs/decision-log.md` is cheap and targeted. The document doesn't need to be in the proactive read order — it's a reactive search target for institutional memory.

5. **JSON's structural advantage is real for LLMs themselves** — not just external tooling. `"code_roots": ["src/transport"]` carries less ambiguity than the equivalent prose.

6. **Optimize for the failure modes hardest to recover from**: broken invariants > wrong location > arbitrary resolution > convention drift > re-litigation > context exhaustion. Hierarchy is stronger on the severe ones; density is stronger on the recoverable ones.

## Scope

Repository structure, documentation infrastructure, and agent-operability tooling. Does **not** cover Rust implementation changes, features, or test coverage (those are tracked in `evaluations/REMEDIATION_PLAN.md`).

## Design principle

**Add institutional memory and contextual precision without dismantling axon's onboarding density.** The root AGENTS.md remains the single-read orientation layer. New artifacts are either auto-loaded contextually (nested AGENTS.md) or serve as reactive search targets (decision log, open questions).

---

## Phase 1: Foundation — Status Taxonomy, Escalation, Institutional Memory

**Goal**: Establish document authority framework and institutional memory that all subsequent phases reference.

### D-1.1: Add document authority section to AGENTS.md

Add after "Status" section:

- **Status taxonomy**: `Normative` (binding), `Draft` (guidance, not binding), `Archived` (historical only).
- **Authority hierarchy**: `spec/*` > `AGENTS.md` / `CONTRIBUTING.md` > `README.md` > code comments.
- **Escalation rule**: If normative sources conflict or implementation disagrees with spec, stop and request clarification. Do not silently choose one interpretation.
- **Status matrix**: table listing every document and its current status.

### D-1.2: Add status headers to all authoritative documents

Add `Status: Normative` or `Status: Draft` as the first line after the title in:

| Document | Status |
|---|---|
| `spec/SPEC.md` | Normative |
| `spec/MESSAGE_TYPES.md` | Normative |
| `spec/WIRE_FORMAT.md` | Normative |
| `spec/IPC.md` | Normative |
| `AGENTS.md` | Normative |
| `CONTRIBUTING.md` | Normative |
| `rubrics/QUALITY.md` | Normative |
| `rubrics/DOCUMENTATION.md` | Normative |
| `rubrics/ALIGNMENT.md` | Normative |

### D-1.3: Create `docs/decision-log.md`

Seed with key historical decisions currently implicit in the codebase:

- DEC-001: Ed25519 over other curves (simplicity, LLM-first, single-purpose)
- DEC-002: SHA-256 for agent ID derivation (deterministic, collision-resistant, no truncation)
- DEC-003: QUIC over TCP (multiplexed streams, built-in TLS, connection migration)
- DEC-004: mDNS for LAN discovery (zero-config, no coordination server)
- DEC-005: Peer pinning model (reject unknown peers at TLS layer)
- DEC-006: Base64-encoded identity.key format (replacing legacy raw format)
- DEC-007: Bounded IPC queues with overflow-disconnect (backpressure over silent drop)
- DEC-008: 4 fixed message kinds at protocol level (extensibility via payload, not new kinds)

### D-1.4: Create `docs/open-questions.md`

Seed from any TODO/FIXME items in code or unresolved design questions.

### D-1.5: Wire into AGENTS.md

Update the read-order in "Specs to Read First" to include:

```
6. docs/decision-log.md — prior architectural decisions (search before proposing alternatives)
7. docs/open-questions.md — unresolved ambiguities (do not silently resolve)
```

Add co-change rule to "Key Invariants" section:

> When making an architectural decision, record it in `docs/decision-log.md`. When encountering an ambiguity that cannot be resolved from existing normative documents, log it in `docs/open-questions.md`.

### Acceptance criteria

- Every document in the status matrix has a matching `Status:` header.
- `docs/decision-log.md` exists with ≥8 seed entries.
- `docs/open-questions.md` exists with ≥1 seed entry.
- `AGENTS.md` contains status matrix, authority hierarchy, escalation rule, and updated read-order.
- `make verify` passes.

---

## Phase 2: Machine-Readable Agent Index + Nested AGENTS.md

**Goal**: Enable programmatic task routing AND contextual subsystem guidance.

**Rationale**: These are coupled because they serve the same purpose at different granularities — the agent index routes tasks to subsystems, nested AGENTS.md provides subsystem-specific guardrails once there.

### D-2.1: Create `docs/agent-index.json`

Machine-readable index with subsystems, code_roots, test_roots, key_files, specs, validation commands, and common_tasks. Subsystems:

| ID | code_roots | key_files |
|---|---|---|
| `transport` | `axon/src/transport` | `tls.rs`, `quic_transport.rs`, `connection.rs` |
| `message` | `axon/src/message` | `envelope.rs` |
| `ipc` | `axon/src/ipc` | `protocol.rs`, `server.rs`, `client_handler.rs`, `auth.rs` |
| `daemon` | `axon/src/daemon` | `mod.rs`, `command_handler.rs`, `reconnect.rs`, `peer_events.rs` |
| `discovery` | `axon/src/discovery` | `mod.rs` |
| `identity` | `axon/src/identity` | `mod.rs` |
| `peer_table` | `axon/src/peer_table` | `mod.rs` |
| `peer_token` | `axon/src/peer_token` | `mod.rs` |
| `config` | `axon/src/config` | `mod.rs` |
| `cli` | `axon/src/app` | `run.rs`, `doctor/mod.rs`, `examples.rs`, `cli/` |

Common tasks: envelope-change, ipc-command-change, tls-change, cli-command-change, config-key-change, doctor-check-change, discovery-change, daemon-lifecycle-change.

### D-2.2: Create nested AGENTS.md for all 10 modules

Each file: 15–30 lines covering priorities, file responsibilities, guardrails, and test targets. Consistency requires all modules, even simple ones.

| File | Key content |
|---|---|
| `axon/src/transport/AGENTS.md` | TLS security > correctness > performance; never weaken pinning; one-message-per-stream; framing must match `spec/WIRE_FORMAT.md` |
| `axon/src/message/AGENTS.md` | Spec compliance first; envelope schema must match `spec/WIRE_FORMAT.md`; 4 kinds are fixed; unknown-field tolerance required |
| `axon/src/ipc/AGENTS.md` | Command semantics must match `spec/IPC.md`; bounded queues overflow-disconnect, not silent drop; validate before forwarding |
| `axon/src/daemon/AGENTS.md` | Lightweight router only — no protocol logic; bounded resource usage; reconnect backoff preserved; lockfile semantics |
| `axon/src/discovery/AGENTS.md` | mDNS service type `_axon._udp.local.`; TXT record format is normative; discovered peers must go through PeerTable |
| `axon/src/identity/AGENTS.md` | Ed25519 only; agent_id = SHA-256(pubkey); identity.key is base64-encoded seed; reject non-base64/legacy formats |
| `axon/src/peer_table/AGENTS.md` | PeerTable owns PubkeyMap — no manual sync; at most one non-static peer per address; static peers block discovered peers at same address |
| `axon/src/peer_token/AGENTS.md` | Token format: `axon://<pubkey_base64url>@<host>:<port>`; round-trip encode/decode must be tested |
| `axon/src/config/AGENTS.md` | YAML parsing; co-change: update README.md Configuration Reference tables; hostname resolution at load time |
| `axon/src/app/AGENTS.md` | Binary-only code; never import `app::` from library modules; CLI changes → update `cli_contract.rs`; doctor changes → update `doctor_contract.rs` |

### D-2.3: Add nested AGENTS index to root AGENTS.md

List all nested AGENTS.md files and their scope. Add maintenance rule: "when adding or renaming subsystem directories, update this index and the affected nested AGENTS.md files in the same change."

### D-2.4: Add co-change rule to CONTRIBUTING.md

> When adding, removing, or renaming modules, update `docs/agent-index.json` in the same change.

### Acceptance criteria

- `docs/agent-index.json` is valid JSON; all paths resolve to existing files/directories.
- 10 nested AGENTS.md files exist, each ≤35 lines.
- Root AGENTS.md contains nested-AGENTS index.
- CONTRIBUTING.md includes agent-index co-change rule.
- `make verify` passes.

---

## Phase 3: Evaluation Infrastructure

**Goal**: Upgrade rubric infrastructure for rigor, traceability, and de-duplication.

### D-3.1: Create `rubrics/EVALUATION-PRINCIPLES.md`

Factor out the "Evaluation principles" section currently duplicated across all 3 rubrics into a shared file. Content:

- Evidence, not intent (every deduction cites verifiable evidence).
- Material impact only.
- Risk-weighted scoring.
- No rubber stamps (100/100 = zero unresolved material risk).
- Severity calibration: critical / major / moderate / minor.
- Required review output format.

Update each rubric to reference it:

```markdown
## Evaluation principles

Apply [`EVALUATION-PRINCIPLES.md`](EVALUATION-PRINCIPLES.md).
```

### D-3.2: Create `rubrics/README.md`

Rubric suite index with:

- **Concern ownership map**: which rubric is primary owner for each concern (prevents double-deduction).

| Concern | Primary rubric | Secondary |
|---|---|---|
| Protocol correctness / wire behavior | QUALITY | DOCUMENTATION |
| Security / TLS / input validation | QUALITY | ALIGNMENT |
| Spec accuracy / interop documentation | DOCUMENTATION | QUALITY |
| LLM navigability / naming / structure | ALIGNMENT | DOCUMENTATION |
| Agent/repo operability | AGENT-READABILITY | ALIGNMENT |

- **Scoring policy**: minimum 70/100 per rubric (matching existing PR gate); critical-finding veto.
- **Double-deduction guidance**: one defect may touch multiple rubrics; score in primary, note in secondary.
- **Rubric index table** with statuses.
- **Spec-to-rubric traceability** (inline section rather than separate file — 4 specs × 4 rubrics is small enough):

| Spec section | Topic | Primary rubric |
|---|---|---|
| `WIRE_FORMAT.md` §2 | Identity / crypto binding | QUALITY §1, §2 |
| `WIRE_FORMAT.md` §3 | QUIC connection lifecycle | QUALITY §1, §2 |
| `WIRE_FORMAT.md` §4–5 | Stream lifecycle / framing | QUALITY §1 |
| `WIRE_FORMAT.md` §6 | JSON encoding / envelope | QUALITY §1 |
| `WIRE_FORMAT.md` §7 | Peer pinning / reconnection | QUALITY §1, §2 |
| `WIRE_FORMAT.md` §9 | Error handling on wire | QUALITY §5 |
| `WIRE_FORMAT.md` §10 | IPC wire protocol | QUALITY §1 |
| `WIRE_FORMAT.md` §11 | mDNS discovery | QUALITY §1 |
| `MESSAGE_TYPES.md` | Message kinds / stream mapping | QUALITY §1; DOCUMENTATION §1 |
| `IPC.md` §3 | IPC commands | QUALITY §1; DOCUMENTATION §1 |
| `IPC.md` §4 | Error codes | QUALITY §5 |
| `IPC.md` §5 | Inbound events | QUALITY §1 |
| `SPEC.md` §1–2 | Identity / discovery | QUALITY §1, §2 |
| `SPEC.md` §3 | Transport (QUIC) | QUALITY §1, §4 |
| `SPEC.md` §4 | Message format | DOCUMENTATION §1 |
| `SPEC.md` §5 | Local IPC | QUALITY §1; DOCUMENTATION §1 |
| `SPEC.md` §6 | CLI | DOCUMENTATION §2, §5 |

### D-3.3: Create `rubrics/AGENT-READABILITY.md`

100-point rubric evaluating the repo from an LLM agent's perspective:

1. **Index health and navigation aids** (20) — agent index exists and is current; read-order present; subsystem boundaries clear; validation commands provided.
2. **Context-budget discipline** (20) — 500-line limit respected; modules shallow; boilerplate minimized; test entrypoints discoverable.
3. **Documentation drift and normative coherence** (20) — authority hierarchy explicit; status headers match matrix; decision log maintained; spec requirements traceable.
4. **Cross-reference and path integrity** (15) — links resolve; agent-index paths exist; subsystem boundaries match module layout; validation commands work.
5. **Tidiness and noise control** (15) — no orphaned code; clippy clean; TODOs centralized in open-questions; no duplicated guidance.
6. **Agent-operability guardrails** (10) — nested AGENTS.md present; hard constraints enforceable; co-change rules enumerated; common-task golden paths maintained.

### D-3.4: Update AGENTS.md status matrix

Add entries for all new rubric files.

### Acceptance criteria

- `rubrics/EVALUATION-PRINCIPLES.md` exists; 3 existing rubrics reference it.
- `rubrics/README.md` exists with concern ownership map, scoring policy, and spec traceability.
- `rubrics/AGENT-READABILITY.md` exists with 6 scored categories totaling 100 points.
- AGENTS.md status matrix includes all rubric files.
- `make verify` passes.

---

## Phase summary

| Phase | Deliverables | New files | Modified files |
|---|---|---|---|
| 1 | Status taxonomy, escalation, decision log, open questions | 2 (`docs/`) | ~12 (status headers) + AGENTS.md |
| 2 | Agent index, 10 nested AGENTS.md | 11 | AGENTS.md, CONTRIBUTING.md |
| 3 | Evaluation principles, rubrics README, agent-readability rubric | 3 | 3 rubrics + AGENTS.md |
| **Total** | | **16 new files** | **~16 modified files** |

## What was intentionally excluded (and why)

| Pattern | Reason for exclusion |
|---|---|
| Agent quickstart overlay | Axon's root AGENTS.md density IS the quickstart; a separate file would dilute the strength. |
| `repo-contract.yaml` + validation script | Makefile already enforces key constraints; adds maintenance surface without proportional value at current scale. Revisit if CI needs structural validation. |
| JSON evaluation template | 4 rubrics don't need JSON schema ceremony. Structured markdown with the evaluation principles is sufficient. |
| Trace matrix as separate file | 4 specs × 4 rubrics is small enough to be an inline section in `rubrics/README.md`. |
| Constants index as separate file | Already maintained as README.md "Internal constants" table with a co-change rule in AGENTS.md. |

## Maintenance

Record each phase completion as an entry in `docs/decision-log.md`.
