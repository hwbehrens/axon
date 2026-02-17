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
| Peer table / pinning / PubkeyMap | `axon/src/peer_table.rs` |
| mDNS / static discovery | `axon/src/discovery.rs` |
| Daemon event loop / startup / shutdown | `axon/src/daemon/mod.rs` |
| Command dispatch | `axon/src/daemon/command_handler.rs` |
| Discovery event handling | `axon/src/daemon/peer_events.rs` |
| Reconnection logic | `axon/src/daemon/reconnect.rs` |
| CLI commands | `axon/src/main.rs` |
| CLI example output | `axon/src/cli_examples.rs` |
| Ed25519 identity / agent ID | `axon/src/identity.rs` |
| Config file parsing | `axon/src/config.rs` |

## Invariants

Do not break these. They are load-bearing:

- **Agent ID = SHA-256(pubkey)** — the `from` field must match the public key in the peer's TLS certificate. Reject on mismatch.
- **Peer pinning** — unknown peers must not be accepted at TLS/transport; peers must be in the peer table before connection.
- **PeerTable owns the pubkey map** — TLS verifiers read from PeerTable's shared PubkeyMap. No manual sync needed.

## Verification

Run all three before submitting. All must pass:

```sh
cd axon
cargo fmt                           # format
cargo clippy -- -D warnings         # lint — must be warning-free
cargo test                          # all tests must pass
```

## Constraints

### File size

All source files must stay **under 500 lines**. If a file approaches this limit, split it into a subdirectory module. This ensures any file can be parsed in a single read.

### Code style

- Follow existing conventions — look at neighboring files before writing new code.
- Use existing libraries and utilities. Do not add new dependencies without justification.
- Semantic field names: `question` not `q`, `report_back` not `rb`. LLMs infer meaning from names.
- No comments unless the code is genuinely complex. The code should be self-documenting.
- Prefer separating mechanical refactors (file splits, renames) from functional changes into distinct commits when possible.

### Commit messages

- State the user-visible behavior change in the subject line, not just what code was touched.
- Note spec impact when applicable (e.g., "IPC: reject inbox limit outside 1–1000 (IPC.md §3.3)").
- Separate mechanical refactors from functional changes into distinct commits.

### Security

- Never log or expose private keys, secrets, or sensitive data.
- All crypto uses established crates (`ed25519-dalek`, `quinn`, `rustls`). No hand-rolled crypto.

## Testing Requirements

Every change must include tests. The test structure:

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
