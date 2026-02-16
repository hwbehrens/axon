# Contributing to AXON

AXON is built by and for LLM agents. This guide is written accordingly — concise, structured, and machine-parseable. No ambiguity, no filler.

## Before You Start

Read these in order:

1. [`spec/SPEC.md`](./spec/SPEC.md) — protocol architecture (QUIC, Ed25519, discovery, lifecycle)
2. [`spec/MESSAGE_TYPES.md`](./spec/MESSAGE_TYPES.md) — all message kinds, payload schemas, stream mapping
3. [`spec/WIRE_FORMAT.md`](./spec/WIRE_FORMAT.md) — normative wire format for interoperable implementations
4. [`spec/IPC.md`](./spec/IPC.md) — IPC protocol, Unix socket commands, auth, receive buffer
5. [`AGENTS.md`](./AGENTS.md) — module map, key invariants, recipes, testing requirements

The spec is authoritative. If the implementation disagrees with the spec, the spec wins.

## Module Map

Know where to make changes before you start editing:

| Change | File(s) |
|--------|---------|
| Envelope schema / payloads | `axon/src/message/envelope.rs`, `axon/src/message/payloads.rs` |
| Add/change a message kind | See [recipe](#adding-a-new-message-kind) |
| QUIC framing / max message size | `axon/src/message/wire.rs`, `axon/src/transport/framing.rs` |
| TLS peer verification | `axon/src/transport/tls.rs` |
| Hello handshake / version negotiation | `axon/src/transport/handshake.rs` |
| IPC command/reply schema | `axon/src/ipc/protocol.rs` |
| IPC server behavior | `axon/src/ipc/server.rs` |
| IPC dispatch + hello/auth/req_id gating | `axon/src/ipc/handlers/mod.rs` |
| IPC hello + auth handlers | `axon/src/ipc/handlers/hello_auth.rs` |
| IPC v2 commands (whoami/inbox/ack/subscribe) | `axon/src/ipc/handlers/commands.rs` |
| IPC inbound broadcast fanout | `axon/src/ipc/handlers/broadcast.rs` |
| IPC receive buffer | `axon/src/ipc/receive_buffer.rs` |
| IPC peer credential auth | `axon/src/ipc/auth.rs` |
| IPC backend trait | `axon/src/ipc/backend.rs` |
| Peer table operations | `axon/src/peer_table.rs` |
| mDNS / static discovery | `axon/src/discovery.rs` |
| Daemon orchestration | `axon/src/daemon/mod.rs` |
| Reconnection logic | `axon/src/daemon/reconnect.rs` |
| CLI commands | `axon/src/main.rs` |
| Ed25519 identity / agent ID | `axon/src/identity.rs` |
| Config file parsing | `axon/src/config.rs` |

## Invariants

Do not break these. They are load-bearing:

- **Hello must be first** — no messages sent or accepted until the `hello` handshake completes on a QUIC connection.
- **Agent ID = SHA-256(pubkey)** — the `from` field must match the public key in the peer's TLS certificate. Reject on mismatch.
- **Peer pinning** — `set_expected_peer()` must be called before a peer can connect. Unknown peers are rejected at TLS.
- **Lower agent_id initiates** — the peer with the lexicographically lower agent_id opens the QUIC connection. Prevents duplicates.
- **Replay protection** — inbound message UUIDs are tracked in a TTL cache. Duplicates are dropped.

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

Located in `axon/tests/`. Test cross-module interactions: IPC → transport → IPC routing, discovery → connection → handshake flows.

### Spec compliance tests

In `axon/tests/spec_compliance.rs`. Every message kind must have a round-trip serialization test validating against the spec.

### Property-based tests

Use `proptest` in `*_tests.rs` files. Test invariants over randomly generated inputs (round-trip encode/decode, concurrent operations, config precedence). Commit any `proptest-regressions/` files.

### Fuzz targets

In `axon/fuzz/fuzz_targets/`. When adding a new deserialization entry point, add a corresponding fuzz target.

### What NOT to test

Don't test third-party crate internals (`quinn`, `ed25519-dalek`, `mdns-sd`). Test your integration with them, not their correctness.

## Adding a New Message Kind

Follow this recipe exactly:

1. **Add variant to `MessageKind`** in `axon/src/message/kind.rs`
   - Update `expects_response()`, `is_response()`, `is_required()`, and `Display`
2. **Add payload struct** in `axon/src/message/payloads.rs` with serde derives
3. **Update `hello_features()`** in `axon/src/message/kind.rs` if the kind is optional
4. **Update `auto_response()`** in `axon/src/transport/handshake.rs` if the daemon should auto-reply
5. **Add CLI command** in `axon/src/main.rs` if exposed to users
6. **Add serde round-trip test** in `axon/src/message/payloads.rs`
7. **Add spec compliance test** in `axon/tests/spec_compliance.rs`
8. **Update `spec/MESSAGE_TYPES.md`** with the new kind's schema

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE).
