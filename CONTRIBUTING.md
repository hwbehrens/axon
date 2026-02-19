# Contributing to AXON

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
| QUIC bind / connect / send | `axon/src/transport/quic_transport.rs` |
| Connection loop / framing | `axon/src/transport/connection.rs` |
| IPC command/reply schema | `axon/src/ipc/protocol.rs` |
| IPC server behavior / broadcast | `axon/src/ipc/server.rs` |
| IPC peer credential auth | `axon/src/ipc/auth.rs` |
| Peer table / pinning / PubkeyMap | `axon/src/peer_table/` |
| mDNS / static discovery | `axon/src/discovery/` |
| Daemon event loop / startup / shutdown | `axon/src/daemon/mod.rs` |
| Command dispatch | `axon/src/daemon/command_handler.rs` |
| Discovery event handling | `axon/src/daemon/peer_events.rs` |
| Reconnection logic | `axon/src/daemon/reconnect.rs` |
| CLI commands | `axon/src/app/run.rs` |
| Doctor diagnostics | `axon/src/app/doctor/` |
| CLI example output | `axon/src/app/examples.rs` |
| Ed25519 identity / agent ID | `axon/src/identity/` |
| Config file parsing | `axon/src/config/` |

## Invariants

Do not break these. They are load-bearing:

- **Agent ID = SHA-256(pubkey)** — the `from` field must match the public key in the peer's TLS certificate. Reject on mismatch.
- **Peer pinning** — unknown peers must not be accepted at TLS/transport; peers must be in the peer table before connection.
- **Address uniqueness** — at most one non-static peer per network address; when a new identity appears at an existing address, the stale entry is evicted from the PeerTable. Discovered/cached peers are blocked from inserting when a static peer already occupies the address.
- **PeerTable owns the pubkey map** — TLS verifiers read from PeerTable's shared PubkeyMap. No manual sync needed.

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

Files like `daemon/reconnect.rs` or `transport/tls.rs` are fine as single files inside their parent directory. If a leaf submodule grows and needs splitting, promote it to its own subdirectory (`tls/mod.rs` + children) without affecting sibling modules.

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
  Example: `peer_table/tests/{mod.rs, basic.rs, eviction.rs, proptest.rs}`.
#### Naming conventions

- Module directories use **snake_case**: `peer_table/`, `peer_token/`.
- Files inside a directory do **not** repeat the module name: use `tests/basic.rs`, not `tests/peer_table_basic.rs`.
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
- If you change persisted files or on-disk formats (`identity.key`, `identity.pub`, `known_peers.json`, `config.yaml` semantics), document reset/re-init guidance in the same PR (README/spec/release notes as appropriate).
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

Use `proptest` in `*_tests.rs` files. Test invariants over randomly generated inputs (round-trip encode/decode, concurrent operations, config precedence). Commit any `proptest-regressions/` files.

### Fuzz targets

In `axon/fuzz/fuzz_targets/`. When adding a new deserialization entry point, add a corresponding fuzz target.

### What NOT to test

Don't test third-party crate internals (`quinn`, `ed25519-dalek`, `mdns-sd`). Test your integration with them, not their correctness.

## Message Kinds

Message kinds are fixed at the protocol level (`request`, `response`, `message`, `error`). Do not add new kinds without updating the spec. New application-level semantics should be expressed via message payload content, not new kinds.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE).
