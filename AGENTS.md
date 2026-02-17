# AXON — Agent eXchange Over Network

LLM-first local messaging protocol + Rust daemon/CLI for secure agent-to-agent communication over QUIC.

## Status

Working implementation. The Rust crate in `axon/` includes the daemon, CLI, IPC, QUIC transport, mDNS discovery, static peer config, and a full test/fuzz/bench harness. Specs in `spec/` are authoritative; if implementation disagrees, the spec wins.

## Repository Layout

```
README.md                  Project overview, quickstart, docs index
AGENTS.md                  This file (LLM agent onboarding/orientation)
CONTRIBUTING.md            Contribution workflow, full module map, invariants, testing requirements
LICENSE

spec/                      Protocol specifications (authoritative)
  SPEC.md                  Architecture + lifecycle (QUIC, identity, discovery, transport)
  MESSAGE_TYPES.md         Message kinds (4) + stream mapping
  WIRE_FORMAT.md           Normative interoperable wire format
  IPC.md                   IPC protocol, Unix socket commands

rubrics/                   Evaluation rubrics (quality, documentation, alignment)

axon/                      Rust implementation (Cargo crate)
  Cargo.toml               Dependencies and package metadata (Rust 2024 edition)
  Makefile                 Canonical build/test/verify entrypoints
  src/                     Implementation
    main.rs                CLI entrypoint (subcommands: daemon, send, notify, peers, status, identity, whoami, examples)
    lib.rs                 Crate root
    daemon/                Daemon orchestration, lifecycle, reconnect
    discovery.rs           mDNS + static peer discovery (plain async functions)
    identity.rs            Ed25519 identity + agent_id derivation
    config.rs              TOML config parsing (name, port, peers)
    ipc/                   Unix socket IPC protocol + server
    message/               MessageKind (4 variants), Envelope, encode/decode
    transport/             QUIC/TLS, connections, framing
    peer_table.rs          Peer storage, pinning, shared PubkeyMap
  tests/                   Integration, spec compliance, adversarial, e2e tests
  benches/                 Criterion benchmarks
  fuzz/                    cargo-fuzz harness + fuzz_targets/
  proptest-regressions/    Persisted proptest failures (commit these)
```

## Key Architecture

```
Client (OpenClaw/CLI) ←→ [Unix Socket IPC] ←→ AXON Daemon ←→ [QUIC/UDP] ←→ AXON Daemon ←→ [Unix Socket IPC] ←→ Client
```

- **Identity**: Ed25519 signing keypair. Agent ID derived from SHA-256 of public key. Self-signed X.509 cert generated on each startup for QUIC TLS.
- **Discovery**: mDNS (`_axon._udp.local.`) broadcasts agent ID and public key. Static peers via config file for Tailscale/VPN. Plain async functions.
- **Transport**: QUIC via `quinn`. TLS 1.3 with forward secrecy. Unidirectional streams for fire-and-forget messages, bidirectional streams for request/response.
- **IPC**: Unix domain socket at `~/.axon/axon.sock`. Line-delimited JSON. 4 commands: `send`, `peers`, `status`, `whoami`. Inbound messages are broadcast to connected clients; lagging clients are disconnected when bounded IPC queues overflow.
- **Messages**: JSON envelopes with UUID, kind, payload, and optional ref. 4 kinds: `request`, `response`, `message`, `error`.

## Module Map (summary)

Use this to navigate quickly; for the full "change → file(s)" table, see `CONTRIBUTING.md`.

- **Daemon lifecycle / reconnection**: `axon/src/daemon/`
- **Discovery (mDNS + static peers, plain functions)**: `axon/src/discovery.rs`
- **Transport (QUIC/TLS/connections/framing)**: `axon/src/transport/`
- **Message kinds + envelopes + encode/decode**: `axon/src/message/`
- **IPC protocol + server**: `axon/src/ipc/`
- **Identity + agent_id derivation**: `axon/src/identity.rs`
- **Config parsing**: `axon/src/config.rs`
- **Peer table + pinning**: `axon/src/peer_table.rs`
- **CLI**: `axon/src/main.rs`

## Key Invariants (summary)

These are load-bearing. Do not change behavior without updating spec + tests. Full list: `CONTRIBUTING.md`.

- **Configuration reference**: when adding or changing a configurable setting (in `Config` / `config.toml`) or an internal constant (timeout, limit, interval, etc.), update the Configuration Reference tables in `README.md`.
- **Agent ID = SHA-256(pubkey)**: peer identity must match TLS certificate/public key; reject mismatches.
- **Peer pinning**: unknown peers must not be accepted at TLS/transport; peers must be in the PeerTable's shared PubkeyMap before connection.

## Building & Verification

The `Makefile` in `axon/` is canonical. Run commands from `axon/`.

```sh
cd axon
make check        # fast typecheck
make fmt          # rustfmt
make lint         # clippy -D warnings
make test-unit    # quick unit tests
make test-all     # full test suite
make verify       # fmt + lint + test-all (pre-commit default)
```

Optional (requires additional tooling):

```sh
make coverage         # cargo llvm-cov (summary)
make coverage-html    # HTML report
make fuzz             # cargo-fuzz (nightly)
make mutants-fast     # cargo-mutants focused subset
make mutants          # broader mutation testing (slower)
```

## Testing Conventions

Detailed requirements and recipes live in `CONTRIBUTING.md`. Key conventions:

- **Unit tests live in sibling `*_tests.rs` files**, wired from the module via:
  ```rust
  #[cfg(test)]
  #[path = "foo_tests.rs"]
  mod tests;
  ```
- **Integration/spec/adversarial/e2e tests** are in `axon/tests/`:
  - `make test-integration` — integration + spec compliance + adversarial
  - `make test-e2e` — daemon lifecycle
- **Property-based tests** use `proptest`. Commit `proptest-regressions/` when generated.
- **Fuzz targets** live in `axon/fuzz/fuzz_targets/`. Add one for any new deserialization entrypoint.
- **Mutation testing** via `cargo-mutants` validates test suite quality.
- **File size limit**: all Rust source files (`.rs`) must stay under 500 lines. Split into submodules when approaching.

## Specs to Read First

1. `spec/SPEC.md` — architecture + lifecycle (identity, discovery, transport)
2. `spec/MESSAGE_TYPES.md` — message kinds (4), stream mapping
3. `spec/WIRE_FORMAT.md` — normative interoperable wire format
4. `spec/IPC.md` — IPC protocol, Unix socket commands
5. `CONTRIBUTING.md` — contribution workflow, full module map, invariants, testing requirements
