# API Contract Review

Status: Draft

Review changes to AXON's protocol contracts for backwards compatibility, spec compliance, and consumer impact. Use this checklist when reviewing PRs that modify the IPC protocol, wire format, message envelope schema, or any externally observable behavior.

## Scope

AXON has three contract surfaces:

1. **Wire format** (`spec/WIRE_FORMAT.md`) — QUIC stream JSON envelopes between daemons
2. **IPC protocol** (`spec/IPC.md`) — Unix socket JSON commands between clients and daemon
3. **Message types** (`spec/MESSAGE_TYPES.md`) — The four message kinds and their stream mapping

Changes to any of these surfaces affect interoperability between AXON implementations and between client tools and the daemon.

## Checklist

### 1. Breaking Change Detection

- [ ] No required fields added to envelope schema without a protocol version bump
- [ ] No fields removed from IPC responses that existing clients may parse
- [ ] No field type changes (string → number, nullable → non-nullable, etc.)
- [ ] No changes to IPC command names or required parameters
- [ ] No changes to error response structure (`ok`, `error` fields)
- [ ] No changes to exit codes that CLI consumers may handle
- [ ] No changes to message kind semantics (`request`, `response`, `message`, `error`)
- [ ] No removal of IPC commands that clients may send
- [ ] No changes to stream directionality (uni vs bidi) for existing message kinds

### 2. Additive Change Safety

- [ ] New optional fields in IPC commands have sensible defaults
- [ ] New fields in IPC responses won't confuse strict JSON parsers
- [ ] New IPC commands don't conflict with existing command names
- [ ] New envelope fields are documented in `spec/WIRE_FORMAT.md`
- [ ] New IPC commands are documented in `spec/IPC.md`

### 3. Wire Format Compliance

- [ ] Envelope JSON structure matches `spec/WIRE_FORMAT.md` exactly
- [ ] Agent ID format is `ed25519.<32 hex chars>` (40 chars total)
- [ ] Message size stays within `MAX_MESSAGE_SIZE` (64 KB)
- [ ] IPC line length stays within `MAX_IPC_LINE_LENGTH` (64 KB)
- [ ] Stream-per-message rule is preserved (one envelope per QUIC stream)
- [ ] FIN-delimited framing is preserved (no length-prefix framing)

### 4. Spec-Implementation Agreement

- [ ] Implementation constants match spec-stated values
- [ ] Error responses match spec-documented shapes
- [ ] IPC command parameters match spec-documented schemas
- [ ] Timeout defaults match spec and README Configuration Reference
- [ ] Behavioral edge cases (unknown commands, oversize messages) match spec

### 5. Cross-Implementation Interoperability

- [ ] A non-Rust implementation reading `spec/WIRE_FORMAT.md` would produce compatible output
- [ ] No Rust-specific serialization quirks leak into the wire format
- [ ] No platform-specific assumptions (endianness, path formats) in protocol data
- [ ] Agent ID derivation algorithm is deterministic and language-independent

### 6. Co-Change Requirements

- [ ] `spec/WIRE_FORMAT.md` updated if envelope schema changes
- [ ] `spec/IPC.md` updated if IPC commands/responses change
- [ ] `spec/MESSAGE_TYPES.md` updated if message kind semantics change
- [ ] `README.md` Configuration Reference updated if constants change
- [ ] `docs/agent-index.json` updated if subsystem structure changes
- [ ] `axon/tests/spec_compliance.rs` updated to cover new wire format behavior
- [ ] `axon/tests/cli_contract.rs` updated if CLI output changes
- [ ] Fuzz targets updated if new deserialization entry points are added

### 7. Security Surface

- [ ] No private key material exposed in IPC responses or wire format
- [ ] Peer credential check preserved on IPC socket connections
- [ ] TLS certificate validation unchanged or strengthened
- [ ] Peer pinning invariant preserved (unknown peers rejected at transport)

## Workflow

1. Identify which contract surface is being changed (wire format, IPC, message types)
2. Diff the spec against the implementation change
3. Classify each change: breaking, additive, or internal-only
4. For breaking changes: is there a protocol version bump? Migration path?
5. Check co-change requirements (specs, tests, docs)
6. Verify wire format compliance
7. Document findings using the output format below

## Severity Guide

- **Critical**: Breaking wire format change without version bump; spec and implementation disagree on behavior
- **High**: Breaking IPC change without migration; missing co-change in spec or tests
- **Medium**: Missing spec update for additive change; undocumented new behavior
- **Low**: Documentation drift; inconsistent naming across spec and code
- **Info**: Design improvement suggestions

## Output Format

For each finding:

```
### Finding: [Title]

**Severity:** [Critical / High / Medium / Low / Info]
**Category:** [which checklist section]
**Affected surface:** [wire format / IPC / message types]

#### Description
[What changed and why it matters for interoperability]

#### Evidence
[Spec section vs implementation diff showing the issue]

#### Recommendation
[What should change]
```

Summary table after all findings:

```
| # | Finding | Severity | Category | Affected Surface |
|---|---------|----------|----------|-----------------|
| 1 | ...     | High     | Breaking | IPC             |
```
