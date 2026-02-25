# Rubric Suite Index

Status: Normative

This document defines the concern ownership map, scoring policy, and spec-to-rubric traceability for the rubric suite. Read this before evaluating any individual rubric.

## Concern ownership map

Each concern has one **primary** rubric where the failure is scored, and zero or more **secondary** rubrics that may reference the concern but should not double-deduct the same failure mode.

| Concern | Primary rubric | Secondary |
|---|---|---|
| Protocol correctness / wire behavior | QUALITY | DOCUMENTATION |
| Security / TLS / input validation | QUALITY | ALIGNMENT |
| Spec accuracy / interop documentation | DOCUMENTATION | QUALITY |
| LLM navigability / naming / structure | ALIGNMENT | DOCUMENTATION |
| Testing quality / coverage | QUALITY | — |
| Performance / resource efficiency | QUALITY | ALIGNMENT |
| Agent/repo operability | AGENT-READABILITY | ALIGNMENT |

## Scoring policy

- **Minimum 70/100 per rubric** — matches the existing PR self-assessment gate in `CONTRIBUTING.md`.
- **Critical-finding veto** — any finding rated `critical` (per `EVALUATION-PRINCIPLES.md` severity calibration) blocks regardless of aggregate score.
- Rubrics with `Draft` status are evaluated for awareness but do not block.

## Double-deduction guidance

One code defect may touch multiple rubric concerns. Apply the deduction in the **primary** rubric; in secondary rubrics, note the finding but do not deduct additional points for the same root cause.

### Examples

**Example 1 — Spec drift in envelope validation:**
A bug allows envelopes with an unknown message kind to be forwarded without validation. This is primarily a **QUALITY** finding (§1 Correctness & Protocol Behavior). DOCUMENTATION may note that the spec's forward-compatibility rules should be clarified, but should not independently deduct for the same missing validation.

**Example 2 — Missing config table entry:**
A new config key is added to `Config` but not reflected in README.md's Configuration Reference. This is primarily a **DOCUMENTATION** finding (§2 README & Configuration Reference). ALIGNMENT may note the co-change rule violation, but should not independently deduct for the same missing entry.

## Rubric index

| Rubric | Scope | Status |
|---|---|---|
| [QUALITY.md](QUALITY.md) | Code quality: correctness, security, testing, performance, reliability | Normative |
| [DOCUMENTATION.md](DOCUMENTATION.md) | Specs, README, guides, self-documentation | Normative |
| [ALIGNMENT.md](ALIGNMENT.md) | Project alignment, LLM-first fit, philosophy | Normative |
| [AGENT-READABILITY.md](AGENT-READABILITY.md) | AI/LLM agent navigability of the repository | Normative |

## Spec-to-rubric traceability

Coverage gaps are detectable by diff when specs change.

### `spec/WIRE_FORMAT.md`

| Section | Topic | Primary rubric | Evidence type |
|---|---|---|---|
| §2 | Identity / crypto binding | QUALITY §1, §2 | test |
| §3.1 | UDP port and bind | QUALITY §1 | test |
| §3.2 | TLS peer authentication / pinning | QUALITY §1, §2 | test, adversarial |
| §3.3–3.4 | Keepalives, idle timeout, connection limits | QUALITY §4 | test |
| §4 | Stream lifecycle / one-message-per-stream | QUALITY §1 | test |
| §5 | Message framing / size limits | QUALITY §1 | test, spec compliance |
| §6 | JSON encoding / envelope schema | QUALITY §1; DOCUMENTATION §1 | test, spec compliance |
| §7 | Peer pinning / reconnection | QUALITY §1, §2 | test, integration |
| §9 | Error handling on wire | QUALITY §5 | test |
| §10 | IPC wire protocol | QUALITY §1; DOCUMENTATION §1 | test |
| §11 | mDNS discovery / TXT records | QUALITY §1 | test |
| §12 | Canonical constants | DOCUMENTATION §2 | test |

### `spec/MESSAGE_TYPES.md`

| Section | Topic | Primary rubric | Evidence type |
|---|---|---|---|
| Message Kinds | 4 kinds, forward compatibility | QUALITY §1 | test, spec compliance |
| Envelope | Wire schema, ref handling | QUALITY §1; DOCUMENTATION §1 | test |
| Stream Mapping | Kind-to-stream rules | QUALITY §1 | test |

### `spec/IPC.md`

| Section | Topic | Primary rubric | Evidence type |
|---|---|---|---|
| §2 | Socket security / permissions | QUALITY §2 | test |
| §3 | IPC commands (send, peers, status, whoami, add_peer) | QUALITY §1; DOCUMENTATION §1 | test, CLI contract |
| §4 | Error codes | QUALITY §5 | test |
| §5 | Inbound events / broadcast | QUALITY §1 | test |
| §6 | Multiple clients / bounded queues | QUALITY §1, §4 | test |

### `spec/SPEC.md`

| Section | Topic | Primary rubric | Evidence type |
|---|---|---|---|
| §1 | Identity (Ed25519, agent ID) | QUALITY §1, §2 | test |
| §2 | Discovery (mDNS, static peers) | QUALITY §1 | test, integration |
| §3 | Transport (QUIC) | QUALITY §1, §4 | test |
| §4 | Message format | DOCUMENTATION §1 | spec compliance |
| §5 | Local IPC | QUALITY §1; DOCUMENTATION §1 | test |
| §6 | CLI | DOCUMENTATION §2, §5 | CLI contract |

### Maintenance

When normative spec sections are added, removed, or renumbered, update the traceability tables above in the same change. A spec section with no owning rubric category indicates a coverage gap that should be resolved.

## Cross-references

- Scoring method: [`EVALUATION-PRINCIPLES.md`](EVALUATION-PRINCIPLES.md)
- Quality rubric: [`QUALITY.md`](QUALITY.md)
- Documentation rubric: [`DOCUMENTATION.md`](DOCUMENTATION.md)
- Alignment rubric: [`ALIGNMENT.md`](ALIGNMENT.md)
- Agent-readability rubric: [`AGENT-READABILITY.md`](AGENT-READABILITY.md)
