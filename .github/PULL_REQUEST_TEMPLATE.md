## Description

<!-- What does this PR do and why? -->

## Contribution Checklist

Self-assessment against [`rubrics/QUALITY.md`](../rubrics/QUALITY.md), [`rubrics/DOCUMENTATION.md`](../rubrics/DOCUMENTATION.md), and [`rubrics/ALIGNMENT.md`](../rubrics/ALIGNMENT.md). Check each item you've verified:

### Security & Hardening
- [ ] mTLS peer pinning and identity binding remain correct
- [ ] Handshake / hello-first gating preserved
- [ ] Replay protection unchanged or improved
- [ ] Resource/DoS controls (max message size, connection caps, IPC bounds) preserved
- [ ] No secrets or key material exposed in logs

### Test Quality & Coverage
- [ ] Tests added/updated for every behavior change
- [ ] Invariant-driven assertions (hello gating, pinning, initiator rule, replay dedup)
- [ ] Spec compliance tests updated if schemas/wire behavior changed
- [ ] No flaky or timing-dependent tests introduced

### Performance & Efficiency
- [ ] No unnecessary allocations/copies in hot paths
- [ ] No blocking I/O on the async reactor
- [ ] Bounded concurrency and backpressure preserved

### Maintainability & Code Quality
- [ ] Follows existing code conventions and module boundaries
- [ ] Source files remain under 500 lines
- [ ] No new dependencies without justification

### Operational Maturity
- [ ] Tracing events include peer identifiers and message IDs
- [ ] Config precedence (CLI > config.toml > defaults) preserved if modified
- [ ] Daemon startup/shutdown lifecycle remains clean
- [ ] If CLI/path/persisted-format behavior changed, linked `cli_contract` tests and docs/spec/help updates are included

### Adversarial Robustness
- [ ] Fuzz target added for any new deserialization entrypoint
- [ ] Adversarial tests extended for new attack surface
- [ ] Protocol violations handled safely (no panics on bad input)

### Contribution Hygiene
- [ ] `make verify` passes (fmt + clippy + tests)
- [ ] Spec/docs updated when behavior or schemas change
- [ ] PR is focused — no drive-by refactors

### Interop & Spec Drift Control
- [ ] Wire compatibility preserved (one-message-per-stream, FIN delimiting, max size)
- [ ] Schema changes reflected in `spec/MESSAGE_TYPES.md` and `spec/WIRE_FORMAT.md`

## Self-Assessment Score

<!-- Score your PR against the rubrics in /rubrics (0-100). Be honest — reviewers will validate. -->

**Score: /100**
