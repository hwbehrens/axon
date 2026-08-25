# Contributing to AXON

Status: Normative

AXON is built by and for LLM agents. This guide is written accordingly — concise, structured, and machine-parseable. No ambiguity, no filler.

## Before You Start

Read these in order:

1. [`spec/SPEC.md`](./spec/SPEC.md) — protocol architecture (QUIC, Ed25519, discovery, lifecycle)
2. [`spec/MESSAGE_TYPES.md`](./spec/MESSAGE_TYPES.md) — message kinds and stream mapping
3. [`spec/WIRE_FORMAT.md`](./spec/WIRE_FORMAT.md) — normative wire format for interoperable implementations
4. [`spec/IPC.md`](./spec/IPC.md) — IPC protocol, Unix socket commands
5. [`AGENTS.md`](./AGENTS.md) — module map, key invariants, recipes, testing requirements

The spec is authoritative. If the implementation disagrees with the spec, the spec wins.

## Module Map

Know where to make changes before you start editing:

| Change | File(s) |
|--------|---------|
| Envelope schema / message kinds | `axon/src/message/envelope.rs` |
| TLS peer verification / cert parsing | `axon/src/transport/tls.rs` |
| Connection ownership / QUIC bind / connect / send | `axon/src/transport/quic_transport.rs`, `axon/src/transport/connection_registry.rs` |
| Connection loop / framing | `axon/src/transport/connection.rs` |
| Reconnect attempt versioning / backoff | `axon/src/transport/reconnect.rs` |
| IPC command/reply schema | `axon/src/ipc/protocol.rs` |
| IPC server behavior / broadcast | `axon/src/ipc/server.rs` |
| IPC peer credential auth | `axon/src/ipc/auth.rs` |
| Peer identity / trust / locators / pin snapshots | `axon/src/peer_directory/` |
| Inbound request correlation / handler lease | `axon/src/request_broker/` |
| Bonjour/mDNS candidate discovery | `axon/src/discovery/` |
| Daemon event loop / startup / shutdown | `axon/src/daemon/mod.rs` |
| Command dispatch | `axon/src/daemon/command_handler.rs` |
| CLI commands | `axon/src/app/run.rs` |
| Doctor diagnostics | `axon/src/app/doctor/` |
| CLI example output | `axon/src/app/examples.rs` |
| Ed25519 identity / agent ID | `axon/src/identity/` |
| Config file parsing | `axon/src/config/` |

For machine-readable task routing (subsystem → files → specs → tests), see [`docs/agent-index.json`](./docs/agent-index.json). When adding, removing, or renaming modules, update `docs/agent-index.json` in the same change.

## Invariants

Do not break these. They are load-bearing:

- **Agent ID = SHA-256(pubkey)** — all Agent IDs are canonical validated values; authenticated envelope identity comes from the peer certificate, never wire claims.
- **Intentional admission** — discovery creates candidates only. A key enters the TLS pin set only after explicit enrollment; revocation persists before the pin is removed and the connection is closed.
- **PeerDirectory owns logical peer state** — it is the only mutable owner of identities, trust, configured locators, live observations, and derived immutable pin snapshots/views.
- **ConnectionManager owns physical state** — one authoritative generation-checked slot per peer; deterministic cross-dial selection closes losers, and stale outcomes cannot clear a newer winner.
- **Locator conflicts fail closed** — observations that assign one endpoint to multiple identities are quarantined. Trusted identities are never evicted because an address was reused.
- **RequestBroker owns request correlation** — one IPC handler lease and exactly one terminal outcome per inbound request on its original QUIC stream.

## Verification

Run all three before submitting. All must pass:

```sh
cd axon
cargo fmt                           # format
cargo clippy -- -D warnings         # lint — must be warning-free
cargo test --test cli_contract      # CLI contract gates
cargo test                          # full suite
```

Canonical shortcut:

```sh
cd axon
make verify
```

## Constraints

### File size

All source files must stay **under 500 lines**. If a file approaches this limit, split it into a subdirectory module. This ensures any file can be parsed in a single read.

### Module structure conventions

The codebase follows a strict directory layout. All new modules must conform to these rules.

#### Library vs binary boundary

- **Library code** (reusable daemon/protocol API) lives directly under `axon/src/` and is declared in `lib.rs`. Examples: `config/`, `daemon/`, `transport/`.
- **Binary-only code** (CLI frontend, doctor diagnostics, example output) lives under `axon/src/app/` and is only reachable from `main.rs`. Never import `app::` from library modules.

#### Directory modules, not flat files

Every top-level module under `src/` is a **directory module** (`<name>/mod.rs`), not a flat file (`<name>.rs`). This keeps the `src/` root clean and provides a natural home for tests and future submodules.

When adding a new library module:
1. Create `src/<name>/mod.rs` with the implementation.
2. Add `pub mod <name>;` to `lib.rs`.

When adding a new binary-only module:
1. Create `src/app/<name>.rs` (or `src/app/<name>/mod.rs` if it needs submodules).
2. Add it to `src/app/mod.rs`.

#### Leaf submodules inside a directory may remain single files

Files like `transport/reconnect.rs` or `transport/tls.rs` are fine as single files inside their parent directory. If a leaf submodule grows and needs splitting, promote it to its own subdirectory (`tls/mod.rs` + children) without affecting sibling modules.

#### Test placement

Tests live **inside their module's directory**, not at the `src/` root.

- **Single test file**: place it as `tests.rs` (or `<name>_tests.rs` for leaf submodules) inside the module directory. Wire it from the implementation file:
  ```rust
  #[cfg(test)]
  #[path = "tests.rs"]
  mod tests;
  ```
- **Multiple test files**: place them under a `tests/` subdirectory with an aggregator `tests/mod.rs`. Wire from the implementation:
  ```rust
  #[cfg(test)]
  #[path = "tests/mod.rs"]
  mod tests;
  ```
  Example: `transport/quic_transport_tests/{mod.rs, basic.rs, requests.rs}`.
#### Naming conventions

- Module directories use **snake_case**: `peer_directory/`, `peer_token/`.
- Files inside a directory do **not** repeat the module name: use `tests/basic.rs`, not `tests/peer_directory_basic.rs`.
- Test files for leaf submodules use the `<name>_tests.rs` suffix: `reconnect.rs` → `reconnect_tests.rs`.

#### When to split a file

Split proactively when a file exceeds **~400 lines** or when a logically distinct responsibility can be cleanly separated (e.g., `ipc/server.rs` → `ipc/server.rs` + `ipc/client_handler.rs`). Don't wait until the 500-line limit forces an awkward split.

### Code style

- Follow existing conventions — look at neighboring files before writing new code.
- Use existing libraries and utilities. Do not add new dependencies without justification.
- Semantic field names: `question` not `q`, `report_back` not `rb`. LLMs infer meaning from names.
- No comments unless the code is genuinely complex. The code should be self-documenting.
- Prefer separating mechanical refactors (file splits, renames) from functional changes into distinct commits when possible.

### Pull request self-assessment

Every PR body **must** include a self-assessment score line in the format `**Score: NN/100**`. CI will reject PRs without one, and scores below 70 fail the build. Evaluate your change against the rubrics in [`rubrics/`](./rubrics) to determine the score.

### Commit messages

- State the user-visible behavior change in the subject line, not just what code was touched.
- Note spec impact when applicable (e.g., "IPC: reject inbox limit outside 1–1000 (IPC.md §3.3)").
- Separate mechanical refactors from functional changes into distinct commits.

### Security

- Never log or expose private keys, secrets, or sensitive data.
- All crypto uses established crates (`ed25519-dalek`, `quinn`, `rustls`). No hand-rolled crypto.

## Testing Requirements

Every change must include tests. The test structure:

### Required review gates for user-visible changes

- If you touch CLI parsing/output/routing in `axon/src/app/run.rs`, add or update at least one black-box CLI contract test in `axon/tests/cli_contract.rs`.
- If you change persisted files or on-disk formats (`identity.key`, `identity.pub`, `peers.json`, `config.yaml` semantics), document reset/re-init guidance in the same PR (README/spec/release notes as appropriate).
- If you change behavior shown in CLI help, examples, or spec text, update all affected artifacts in the same PR (`--help`, `README.md`, `spec/`).
- If you change CLI command inventory/help semantics, update docs-conformance coverage (`axon/tests/spec_compliance/cli_help.rs`) as needed.
- If you change `doctor` behavior (CLI wiring or reported checks), update `axon/tests/doctor_contract.rs` to preserve black-box contract coverage.
- For user-visible failure paths, assert both response content and process exit code.

### Unit tests

Every module has `#[cfg(test)]` tests in sibling `*_tests.rs` files, wired via:

```rust
#[cfg(test)]
#[path = "foo_tests.rs"]
mod tests;
```

Cover all public functions, edge cases, and error paths.

### Integration tests

Located in `axon/tests/`. Test cross-module interactions: IPC → transport → IPC routing, discovery → connection → mTLS authentication flows.

### Spec compliance tests

In `axon/tests/spec_compliance.rs`. Message envelope round-trip serialization tests validating against the spec.

### Property-based tests

Two frameworks, selected by property shape (see decision-log DEC-015):

- **`proptest`** is the default for value-level properties — invariants over generated values such as round-trip encode/decode, config precedence, or bound enforcement. Use it in `*_tests.rs` files and commit any `proptest-regressions/` files.
- **`hegeltest`** (Hegel-rust) for stateful, model-based properties — sequences of rules applied to live state with invariants checked per transition (`#[state_machine]`). Its Hypothesis-style shrinking only pays off when counterexamples are rule sequences.

Do not port existing value-level proptests to Hegel absent one of: Hegel reaches a stable release, or stateful properties become the common case. Keep Hegel call sites few while it is beta.

### Testing for known failure classes

Review of the ownership redesign found bugs that conventional coverage and
mutation gates missed. Each maps to a recurring class with a required test
shape; treat this table as a checklist when touching the matching surface.

| Class | Required test shape |
| --- | --- |
| Self-referential fixtures | Persistence/wire formats need at least one fixture transcribed literally from `spec/` text; serde round-trips alone cannot catch field-name drift from the spec. |
| Cancellation safety | Any state inserted before an `.await` whose future can be dropped (select! arms, task aborts, timeouts) needs a test that drops the awaiting task and asserts cleanup. Deadline-guarded state also needs lazy-expiry sweep coverage. |
| Lossy notification channels | A broadcast/laggy channel carrying state-changing notifications (disconnects, losses) needs a reconciliation path plus a test that forces lag and asserts state converges. |
| Resource saturation liveness | Every bounded resource (queues, budgets, leases) needs a test that saturates it and asserts control-plane operations still respond promptly and overflow is rejected with a typed error. Inject small bounds via options rather than choreographing full-scale loads. |
| Half-invariants in stateful tests | Stateful/property suites must assert both directions: forbidden states never occur AND required recoveries/releases do occur (e.g., quarantines release when their cause disappears). |
| Retry semantics | Any automatic retry requires receiver-side idempotency or deduplication for non-idempotent operations, with a test replaying the same request id. |
| Cross-owner race windows | Security- or trust-relevant decisions must re-validate against current authority at the single chokepoint immediately before committing (e.g., enrollment recheck before admitting a connection), with an interleaving test if practical. |

### Fuzz targets

In `axon/fuzz/fuzz_targets/`. When adding a new deserialization entry point, add a corresponding fuzz target.

### What NOT to test

Don't test third-party crate internals (`quinn`, `ed25519-dalek`, `mdns-sd`). Test your integration with them, not their correctness.

## Message Kinds

The four interpreted message kinds are fixed at the protocol level (`request`, `response`, `message`, `error`). Unknown wire strings must be retained losslessly for forward compatibility and answered with `unsupported_kind` on bidirectional streams. Do not add an interpreted kind without updating the spec.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE).
