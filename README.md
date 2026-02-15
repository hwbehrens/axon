<p align="center">
  <img src="axon/assets/icon.png" alt="AXON" width="128" />
</p>

# AXON — Agent eXchange Over Network

A hyper-efficient, LLM-first local messaging protocol for agent-to-agent communication.

## Problem

Current inter-agent communication options are all designed for humans:
- iMessage: rich media, typing indicators, read receipts — none of which LLMs need
- HTTP/REST: stateless, no session awareness, JSON overhead
- OpenClaw sessions_send: works but goes through the gateway abstraction layer

We want something purpose-built for two LLM agents on the same local network.

## Design Principles

1. **Context-budget-aware**: Every message costs tokens. The protocol should minimize unnecessary context consumption.
2. **Structured-first**: No natural language overhead. Payloads are typed, schemaed, and machine-parseable.
3. **Resumable**: Agents restart frequently. The protocol handles reconnection, deduplication, and state recovery.
4. **Minimal round-trips**: Prefer rich single exchanges over chatty back-and-forth.
5. **Zero-trust locally**: Agents authenticate even on LAN (agents have different access levels).

## Status

Working implementation. The daemon, CLI, IPC, QUIC transport, mDNS discovery, and static peer config are all functional. See `spec/` for the protocol specification.

## Quickstart

### Build

```sh
cd axon
cargo build --release
```

The binary is at `axon/target/release/axon`. Add it to your `PATH` or run it directly.

### Run a single daemon

```sh
axon daemon
```

This starts the daemon on the default port (7100), enables mDNS discovery, creates `~/.axon/` with a fresh Ed25519 identity, and listens for IPC commands on `~/.axon/axon.sock`.

### Connect two agents on a LAN

If both machines are on the same local network, mDNS handles everything:

```sh
# Machine A                          # Machine B
axon daemon                          axon daemon
```

Within seconds they discover each other. Verify with:

```sh
axon peers
```

### Connect two agents over Tailscale/VPN (static peers)

When mDNS won't work (different subnets, VPN, etc.), configure peers manually:

```sh
# On each machine, get the identity:
axon identity
# Output: { "agent_id": "ed25519.a1b2c3d4...", "public_key": "base64..." }
```

Then on machine A, create `~/.axon/config.toml`:

```toml
[[peers]]
agent_id = "ed25519.<machine-B-agent-id>"
addr = "<machine-B-ip>:7100"
pubkey = "<machine-B-public-key>"
```

Do the same on machine B with A's info. Then start both daemons:

```sh
axon daemon --disable-mdns    # optional: skip mDNS if not needed
```

### Send a message

```sh
# Query another agent
axon send <agent_id> "What is the capital of France?"

# Delegate a task
axon delegate <agent_id> "Summarize today's news"

# Fire-and-forget notification
axon notify <agent_id> meta.status '{"state":"ready"}'

# See all commands
axon --help
```

### Discover capabilities

```sh
axon discover <agent_id>
```

### Example interaction

```sh
axon examples    # prints a full annotated hello → discover → query → delegate flow
```

## Configuration Reference

All settings are optional. AXON uses sensible defaults; you only need `config.toml` to configure static peers or override defaults.

### `config.toml`

Located at `~/.axon/config.toml` (or `<axon_root>/config.toml`).

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `port` | `u16` | `7100` | QUIC listen port. CLI `--port` overrides this. |
| `max_ipc_clients` | `usize` | `64` | Maximum simultaneous IPC client connections. |
| `max_connections` | `usize` | `128` | Maximum simultaneous QUIC peer connections. |
| `keepalive_secs` | `u64` | `15` | QUIC keepalive interval in seconds. |
| `idle_timeout_secs` | `u64` | `60` | QUIC idle timeout in seconds. Connections with no traffic for this duration are closed. |
| `reconnect_max_backoff_secs` | `u64` | `30` | Maximum backoff between reconnection attempts to unreachable peers. Backoff starts at 1s and doubles. |
| `handshake_timeout_secs` | `u64` | `5` | Maximum time to wait for a hello handshake on a new inbound connection before closing it. |
| `inbound_read_timeout_secs` | `u64` | `10` | Maximum time to wait for data on an inbound QUIC stream before timing out. |

#### Static peers

```toml
[[peers]]
agent_id = "ed25519.<hex>"
addr = "10.0.0.5:7100"
pubkey = "<base64-encoded-ed25519-public-key>"
```

### Internal constants

These are compile-time constants and cannot be changed via configuration.

| Constant | Value | Location | Description |
|----------|-------|----------|-------------|
| `PROTOCOL_VERSION` | `1` | `message/envelope.rs` | Wire protocol version included in every envelope. |
| `MAX_MESSAGE_SIZE` | `65536` (64 KB) | `message/wire.rs` | Maximum encoded envelope size. Messages exceeding this are rejected. |
| `REQUEST_TIMEOUT` | `30s` | `transport/mod.rs` | Timeout for bidirectional request/response exchanges (query, delegate, etc.). |
| `STALE_TIMEOUT` | `60s` | `peer_table.rs` | Discovered (non-static, non-cached) peers with no activity for this duration are removed. |
| `MAX_CLIENT_QUEUE` | `1024` | `ipc/server.rs` | Per-IPC-client outbound message queue depth. Messages are dropped if a client falls behind. |
| `MAX_IPC_LINE_LENGTH` | `256 KB` | `ipc/server.rs` | Maximum length of a single IPC command line. |
| Replay cache TTL | `300s` (5 min) | `daemon/mod.rs` | Duration for which message UUIDs are remembered to detect replays. |
| Replay cache max entries | `100,000` | `daemon/mod.rs` | Maximum replay cache size. Oldest entries are evicted when exceeded. |
| Save interval | `60s` | `daemon/mod.rs` | How often the daemon persists `known_peers.json` to disk. |
| Stale cleanup interval | `5s` | `daemon/mod.rs` | How often the daemon checks for and removes stale discovered peers. |
| Reconnect interval | `1s` | `daemon/mod.rs` | How often the daemon checks for peers needing reconnection. |
| Initial reconnect backoff | `1s` | `daemon/reconnect.rs` | First reconnect attempt delay after a connection failure. Doubles up to `reconnect_max_backoff_secs`. |
| Initiator-rule wait | `2s` | `daemon/command_handler.rs` | When the higher-ID daemon sends a message, it waits this long for the lower-ID peer to initiate a connection. |

## Documentation

| Document | Description |
|----------|-------------|
| [`spec/SPEC.md`](./spec/SPEC.md) | Protocol architecture (QUIC, Ed25519, discovery, lifecycle) |
| [`spec/MESSAGE_TYPES.md`](./spec/MESSAGE_TYPES.md) | All message kinds, payload schemas, stream mapping |
| [`spec/WIRE_FORMAT.md`](./spec/WIRE_FORMAT.md) | Normative wire format for interoperable implementations |
| [`CONTRIBUTING.md`](./CONTRIBUTING.md) | Development guide, module map, testing requirements |
| [`RUBRIC.md`](./RUBRIC.md) | Contribution scoring rubric (100 points across 8 categories) |
| [`evaluations/`](./evaluations/) | Agent evaluation results (not part of the implementation) |
