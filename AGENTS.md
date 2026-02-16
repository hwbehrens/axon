# AXON — Agent eXchange Over Network

LLM-first local messaging protocol + Rust daemon/CLI for secure agent-to-agent communication over QUIC.

## Status

Working implementation. The Rust crate in `axon/` includes the daemon, CLI, IPC, QUIC transport, mDNS discovery, static peer config, and a full test/fuzz/bench harness. Specs in `spec/` are authoritative; if implementation disagrees, the spec wins.

## Repository Layout

```
README.md                  Project overview, quickstart, docs index
AGENTS.md                  This file (LLM agent onboarding/orientation)
CONTRIBUTING.md            Contribution workflow, full module map, invariants, detailed testing requirements
LICENSE

spec/                      Protocol specifications (authoritative)
  SPEC.md                  Architecture + lifecycle (QUIC, identity, discovery, handshake)
  MESSAGE_TYPES.md         Message kinds + payload schemas + stream mapping
  WIRE_FORMAT.md           Normative interoperable wire format

RUBRIC.md                  Contribution scoring rubric (100-point checklist across 8 categories)
evaluations/               Agent evaluation results (not part of the implementation)

axon/                      Rust implementation (Cargo crate)
  Cargo.toml               Dependencies and package metadata (Rust 2024 edition)
  Makefile                 Canonical build/test/verify entrypoints (preferred over raw cargo commands)
  src/                     Implementation
    main.rs                CLI entrypoint (also hosts subcommands)
    lib.rs                 Crate root
    daemon/                Daemon orchestration, lifecycle, reconnect, replay cache
    discovery.rs           mDNS + static peer discovery
    identity.rs            Ed25519 identity + agent_id derivation
    config.rs              TOML config parsing + precedence rules
    ipc/                   Unix socket IPC protocol + server
    message/               Message kinds, envelopes, payloads, wire encoding
    transport/             QUIC/TLS, handshake, framing, connections
    peer_table.rs          Peer storage, pinning, cleanup
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
- **Discovery**: mDNS (`_axon._udp.local.`) broadcasts agent ID and public key. Static peers via config file for Tailscale/VPN. Discovery is trait-based for future extensibility.
- **Transport**: QUIC via `quinn`. TLS 1.3 with forward secrecy. Unidirectional streams for fire-and-forget messages, bidirectional streams for request/response. 0-RTT reconnection.
- **IPC**: Unix domain socket at `~/.axon/axon.sock`. Line-delimited JSON commands (`send`, `peers`, `status`). Inbound messages forwarded to all connected IPC clients.
- **Messages**: JSON envelopes with version, UUID, sender/receiver, timestamp, kind, and payload. Kinds: `hello`, `ping/pong`, `query/response`, `delegate/ack/result`, `notify`, `cancel`, `discover/capabilities`, `error`.

## Module Map (summary)

Use this to navigate quickly; for the full "change → file(s)" table, see `CONTRIBUTING.md`.

- **Daemon lifecycle / reconnection**: `axon/src/daemon/`
- **Discovery (mDNS + static peers)**: `axon/src/discovery.rs`
- **Transport (QUIC/TLS/handshake/framing)**: `axon/src/transport/`
- **Message model + wire encoding**: `axon/src/message/`
- **IPC protocol + server**: `axon/src/ipc/`
- **Identity + agent_id derivation**: `axon/src/identity.rs`
- **Config parsing**: `axon/src/config.rs`
- **Peer table + pinning + replay/dedupe**: `axon/src/peer_table.rs`
- **CLI**: `axon/src/main.rs`

## Key Invariants (summary)

These are load-bearing. Do not change behavior without updating spec + tests. Full list: `CONTRIBUTING.md`.

- **Configuration reference**: when adding or changing a configurable setting (in `Config` / `config.toml`) or an internal constant (timeout, limit, interval, etc.), update the Configuration Reference tables in `README.md`.

- **Hello must be first**: no application messages until QUIC connection completes the `hello` handshake.
- **Agent ID = SHA-256(pubkey)**: peer identity must match TLS certificate/public key; reject mismatches.
- **Peer pinning**: unknown peers must not be accepted at TLS/transport; expected peers must be set before connect.
- **Single-connection rule**: deterministic initiator selection (lower `agent_id` initiates) prevents duplicate links.
- **Replay protection**: inbound message UUIDs are tracked; duplicates are dropped within TTL.

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

1. `spec/SPEC.md` — architecture + lifecycle (identity, discovery, transport, handshake)
2. `spec/MESSAGE_TYPES.md` — message kinds, payload schemas, stream mapping
3. `spec/WIRE_FORMAT.md` — normative interoperable wire format
4. `spec/IPC.md` — IPC protocol, Unix socket commands, auth, receive buffer
5. `CONTRIBUTING.md` — contribution workflow, full module map, invariants, testing requirements
