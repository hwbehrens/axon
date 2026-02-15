# AXON — Agent eXchange Over Network

A lightweight daemon for secure, encrypted, point-to-point messaging between LLM agents on a local network. Agents discover each other via mDNS and communicate over QUIC with TLS 1.3. No human-oriented overhead — every design decision optimizes for token efficiency and machine-parseability.

## Status

The spec (`spec/spec.md`) defines the target architecture. The code in `axon/` has not yet been implemented — `Cargo.toml` has the dependency list ready but there is no source code yet.

## Repository Layout

```
README.md                  Project overview and design principles
AGENTS.md                  This file

spec/                      Protocol specifications
  spec.md                  Protocol spec (QUIC + Ed25519 + TLS 1.3)
  message-types.md         Detailed message kind definitions, payload schemas, and stream mapping

axon/                      Rust implementation (cargo crate)
  Cargo.toml               Dependencies and package metadata
  src/                     (not yet implemented)
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

## Building

```sh
cd axon
cargo build
```

## Testing

```sh
cd axon
cargo test              # unit + integration tests
cargo test -- --ignored # long-running / e2e tests (marked #[ignore])
```

## Development Philosophy

AXON is a small, simple protocol. The implementation must be **rock solid**. Every module must be thoroughly tested before moving on. Do not trade correctness for speed of development.

### Testing Requirements

**Unit tests** — every module must have inline `#[cfg(test)]` unit tests covering:
- All public functions, including edge cases and error paths.
- Serialization/deserialization round-trips for all message types.
- Crypto operations (key generation, signing, certificate creation).
- Peer table operations (insert, remove, stale cleanup, static vs discovered).

**Integration tests** — `axon/tests/` directory, testing interactions between modules:
- IPC command handling: send a JSON command over a Unix socket, verify the daemon's response.
- Discovery → transport flow: peer discovered via mDNS triggers QUIC connection and `hello` exchange.
- Message routing end-to-end within a single process: IPC in → envelope creation → QUIC send → QUIC receive → IPC out.

**End-to-end tests** — marked `#[ignore]`, spin up two real daemon processes on localhost:
- Two daemons discover each other and complete `hello`.
- `axon send` CLI delivers a message from daemon A to daemon B.
- Graceful shutdown: daemon exits cleanly, peer detects disconnection.
- Reconnection: daemon B restarts, daemon A reconnects.

**Future: fuzz and mutation testing** — once the core is stable:
- Fuzz all deserialization entry points (envelope parsing, IPC command parsing, config file parsing) using `cargo-fuzz`.
- Mutation testing via `cargo-mutants` to verify test suite quality — tests should catch mutations in core logic.

### What to Test vs. What Not To

- **Do test**: your own logic, protocol invariants, error handling, state transitions, concurrency (e.g., multiple IPC clients, peer table under contention).
- **Don't test**: third-party crate internals (quinn, ed25519-dalek, mdns-sd). Trust their guarantees. Test your integration with them, not their correctness.

## Specs to Read First

1. `spec/spec.md` — full architecture (QUIC, Ed25519, discovery trait, static peers, lifecycle)
2. `spec/message-types.md` — all message kinds, payload schemas, stream mapping, and domain conventions
