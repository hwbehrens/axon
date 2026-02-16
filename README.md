<p align="center">
  <img src="axon/assets/icon.png" alt="AXON" width="128" />
</p>

<h1 align="center">AXON — Agent eXchange Over Network</h1>

<p align="center">
  A secure, LLM-first messaging protocol for agent-to-agent communication over QUIC.
</p>

<p align="center">
  <a href="https://github.com/hwbehrens/axon/actions/workflows/ci.yml"><img src="https://github.com/hwbehrens/axon/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/hwbehrens/axon/releases/latest"><img src="https://img.shields.io/github/v/release/hwbehrens/axon" alt="Release"></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

<p align="center">
  <a href="./spec/SPEC.md">Spec</a> · <a href="./spec/MESSAGE_TYPES.md">Messages</a> · <a href="./spec/WIRE_FORMAT.md">Wire Format</a> · <a href="./spec/IPC.md">IPC</a> · <a href="./CONTRIBUTING.md">Contributing</a>
</p>

---

## Why?

Every existing communication channel between AI agents was designed for humans:

- **Chat protocols** carry rich media, typing indicators, read receipts, and presence — none of which LLMs need, all of which cost tokens.
- **HTTP/REST** is stateless, unaware of sessions, and burdened with redundant headers.
- **Platform-specific APIs** lock agents into proprietary gateway layers.

AXON is purpose-built for agents talking to agents. It's structured, authenticated, resumable, and context-budget-aware — designed so that agents on the same local network (or across a VPN) can collaborate without wasting tokens on protocol overhead.

## Design Principles

1. **Context-budget-aware** — Every message costs tokens. The protocol minimizes unnecessary context consumption.
2. **Structured-first** — No natural language overhead. Payloads are typed, schema'd, and machine-parseable.
3. **Resumable** — Agents restart frequently. AXON handles reconnection, deduplication, and state recovery automatically.
4. **Minimal round-trips** — Prefer rich single exchanges over chatty back-and-forth.
5. **Zero-trust locally** — Agents authenticate even on LAN. Different agents have different access levels.

## How It Works

```
Agent ←→ [Unix Socket IPC] ←→ AXON Daemon ←→ [QUIC/UDP] ←→ AXON Daemon ←→ [Unix Socket IPC] ←→ Agent
```

Each machine runs a lightweight daemon (<5 MB RSS, negligible CPU when idle). Agents connect to it over a Unix socket and exchange structured JSON messages. The daemon handles everything else:

- **Identity** — Ed25519 keypair generated on first run. Agent ID derived from the public key. Self-signed X.509 cert for QUIC/TLS 1.3.
- **Discovery** — mDNS on LAN (zero-config) or static peers in `config.toml` for VPN/Tailscale setups.
- **Transport** — QUIC with TLS 1.3, forward secrecy, and 0-RTT reconnection.
- **Security** — Mutual peer pinning, hello-first handshake gating, replay protection with UUID deduplication.

## Quickstart

### Build

```sh
cd axon
cargo build --release
```

The binary is at `axon/target/release/axon`. Add it to your `PATH` or run it directly.

### Run

```sh
axon daemon
```

Starts on port 7100, creates `~/.axon/` with a fresh Ed25519 identity, enables mDNS discovery, and listens for IPC on `~/.axon/axon.sock`.

### Connect agents on a LAN

If machines are on the same local network, mDNS handles everything automatically:

```sh
# Machine A                          # Machine B
axon daemon                          axon daemon
```

Within seconds they discover each other. Verify:

```sh
axon peers
```

### Connect agents over Tailscale / VPN

When mDNS isn't available, configure static peers:

```sh
# On each machine, get the identity:
axon identity
# → { "agent_id": "ed25519.a1b2c3d4...", "public_key": "base64..." }
```

Create `~/.axon/config.toml` on each machine with the other's info:

```toml
[[peers]]
agent_id = "ed25519.<peer-agent-id>"
addr = "<peer-ip>:7100"
pubkey = "<peer-public-key>"
```

Then start:

```sh
axon daemon --disable-mdns
```

### Send messages

```sh
# Query another agent
axon send <agent_id> "What is the capital of France?"

# Delegate a task
axon delegate <agent_id> "Summarize today's news"

# Fire-and-forget notification
axon notify <agent_id> meta.status '{"state":"ready"}'

# Discover capabilities
axon discover <agent_id>

# See all commands
axon --help
```

### Example interaction

```sh
axon examples    # prints a full annotated hello → discover → query → delegate flow
```

## Message Types

| Kind | Stream | Purpose |
|------|--------|---------|
| `hello` | Bidirectional | Identity exchange + version negotiation |
| `ping` / `pong` | Bidirectional | Liveness check |
| `query` → `response` | Bidirectional | Ask a question, get an answer |
| `delegate` → `ack` → `result` | Bidir + Unidir | Assign a task, track completion |
| `notify` | Unidirectional | Fire-and-forget information |
| `cancel` → `ack` | Bidirectional | Cancel a pending delegation |
| `discover` → `capabilities` | Bidirectional | Capability negotiation |
| `error` | Bidir or Unidir | Error response or unsolicited error |

See [`spec/MESSAGE_TYPES.md`](./spec/MESSAGE_TYPES.md) for full payload schemas and [`spec/WIRE_FORMAT.md`](./spec/WIRE_FORMAT.md) for the normative wire format.

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
| `ipc.allow_v1` | `bool` | `true` | When `false`, reject IPC v1 (no-hello) connections and require v2+ handshake. |
| `ipc.buffer_size` | `usize` | `1000` | Maximum number of messages in the IPC receive buffer. Set to `0` to disable buffering. |
| `ipc.buffer_ttl_secs` | `u64` | `86400` | Time-to-live for buffered messages in seconds (default: 24 hours). Expired messages are evicted lazily. |
| `ipc.buffer_byte_cap` | `usize` | `4194304` | Approximate byte cap on receive buffer memory (default: 4 MB). Uses envelope size estimates; actual usage may slightly exceed this under adversarial payloads. Oldest messages are evicted when exceeded. |
| `ipc.max_client_queue` | `usize` | `1024` | Per-IPC-client outbound message queue depth. Messages are dropped if a client falls behind. |
| `ipc.token_path` | `String` | `~/.axon/ipc-token` | Path to the IPC auth token file (used when peer credentials are unavailable). |

> **⚠️ Hardened mode note:** The bundled CLI currently uses IPC v1 (no `hello` handshake). Setting `ipc.allow_v1 = false` will cause all CLI commands (`axon peers`, `axon send`, etc.) to be rejected with `hello_required`. Use raw IPC or a v2-capable client when hardened mode is enabled.

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
| `MAX_IPC_LINE_LENGTH` | `64 KB` | `ipc/server.rs` | Maximum length of a single IPC command line. Overlong lines are rejected with `invalid_command`. |
| Replay cache TTL | `300s` (5 min) | `daemon/mod.rs` | Duration for which message UUIDs are remembered to detect replays. |
| Replay cache max entries | `100,000` | `daemon/mod.rs` | Maximum replay cache size. Oldest entries are evicted when exceeded. |
| Save interval | `60s` | `daemon/mod.rs` | How often the daemon persists `known_peers.json` to disk. |
| Stale cleanup interval | `5s` | `daemon/mod.rs` | How often the daemon checks for and removes stale discovered peers. |
| Reconnect interval | `1s` | `daemon/mod.rs` | How often the daemon checks for peers needing reconnection. |
| Initial reconnect backoff | `1s` | `daemon/reconnect.rs` | First reconnect attempt delay after a connection failure. Doubles up to `reconnect_max_backoff_secs`. |
| Initiator-rule wait | `2s` | `daemon/command_handler.rs` | When the higher-ID daemon sends a message, it waits this long for the lower-ID peer to initiate a connection. |
| `IPC_VERSION` | `2` | `ipc/handlers/mod.rs` | Maximum IPC protocol version supported by the daemon. |
| `MAX_CONSUMER_LEN` | `64` bytes | `ipc/handlers/mod.rs` | Maximum length of a consumer name in `hello`. |
| `DEFAULT_BYTE_CAP` | `4194304` (4 MB) | `ipc/receive_buffer.rs` | Default approximate byte cap on receive buffer memory. |
| Max consumers | `1024` | `ipc/receive_buffer.rs` | Maximum number of tracked consumer states before LRU eviction. |

## Documentation

| Document | Description |
|----------|-------------|
| [`spec/SPEC.md`](./spec/SPEC.md) | Protocol architecture — QUIC, Ed25519, discovery, lifecycle |
| [`spec/MESSAGE_TYPES.md`](./spec/MESSAGE_TYPES.md) | All message kinds, payload schemas, stream mapping |
| [`spec/WIRE_FORMAT.md`](./spec/WIRE_FORMAT.md) | Normative wire format for interoperable implementations |
| [`spec/IPC.md`](./spec/IPC.md) | IPC protocol — Unix socket commands, auth, receive buffer |
| [`CONTRIBUTING.md`](./CONTRIBUTING.md) | Development guide, module map, testing requirements |
| [`rubrics/`](./rubrics/) | Evaluation rubrics — quality, documentation, alignment |
| [`SECURITY.md`](./SECURITY.md) | Security policy and vulnerability reporting |

## License

[MIT](./LICENSE)
