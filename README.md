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
2. **Structured-first** — No natural language overhead. Payloads are JSON, machine-parseable.
3. **Resumable** — Agents restart frequently. AXON handles reconnection and peer rediscovery automatically.
4. **Minimal round-trips** — Prefer rich single exchanges over chatty back-and-forth.
5. **Zero-trust locally** — Agents authenticate even on LAN. Different agents have different access levels.

## How It Works

```
Agent ←→ [Unix Socket IPC] ←→ AXON Daemon ←→ [QUIC/UDP] ←→ AXON Daemon ←→ [Unix Socket IPC] ←→ Agent
```

Each machine runs a lightweight daemon (<5 MB RSS, negligible CPU when idle). Agents connect to it over a Unix socket and exchange structured JSON messages. The daemon handles everything else:

- **Identity** — Ed25519 keypair generated on first run. `identity.key` stores a base64-encoded 32-byte seed (strictly required; non-base64 or raw legacy formats are rejected). Agent ID derived from the public key. Self-signed X.509 cert for QUIC/TLS 1.3.
- **Discovery** — mDNS on LAN (zero-config) or static peers in `config.yaml` for VPN/Tailscale setups.
- **Transport** — QUIC with TLS 1.3 and forward secrecy.
- **Security** — Mutual TLS peer pinning — unknown peers rejected at the transport layer.

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
Use `--state-root <DIR>` (aliases: `--state`, `--root`) to override the state directory, or set `AXON_ROOT`.

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
# On each machine, get the share token:
axon identity
# → axon://<pubkey_base64url>@<host-or-ip>:7100
```

Enroll each remote peer token:

```sh
axon connect axon://<pubkey_base64url>@<peer-host-or-ip>:7100
```

Then start:

```sh
axon daemon --disable-mdns
```

### Send messages

```sh
# Send a request to another agent (bidirectional, waits for response)
axon request <agent_id> "What is the capital of France?"

# Override request timeout (seconds)
axon request --timeout 10 <agent_id> "What is the capital of France?"

# Fire-and-forget notification (unidirectional, text payload by default)
axon notify <agent_id> "ready"

# Structured JSON payload for notify
axon notify --json <agent_id> '{"state":"ready"}'

# Enroll a peer from an axon:// token
axon connect axon://<pubkey_base64url>@<host>:<port>

# List peers
axon peers

# Machine-readable peers output
axon peers --json

# Daemon status
axon status

# Machine-readable status output
axon status --json

# Daemon identity (IPC)
axon whoami

# Machine-readable whoami output
axon whoami --json

# Local identity URI (state root files, no daemon required)
axon identity

# Local identity details as JSON
axon identity --json

# One-shot override for URI address output
axon identity --addr my-host.tailnet:7100

# Diagnose local state (read-only report)
axon doctor

# Apply safe local repairs
axon doctor --fix

# Allow identity regeneration if key material is unrecoverable
axon doctor --fix --rekey

# Manage scalar config keys
# (`axon config` follows git-config-style get/set/list/unset/edit conventions)
axon config --list
axon config name alice
axon config --unset name

# See all commands
axon --help
```

### CLI Behavior Contracts

- Global state-root override is available on all commands:
  - `--state-root <DIR>` (aliases: `--state`, `--root`)
  - fallback order: CLI flag -> `AXON_ROOT` -> `~/.axon`
- Exit codes:
  - `0`: success
  - `1`: local/runtime failure after argument parsing (I/O, daemon socket connect/decode, etc.)
  - `2`: CLI parse/usage failure (Clap), daemon/application-level failure reply (`"ok": false`), or `request` remote envelope with `kind=error`
  - `3`: `request` timeout (`{"ok": false, "error": "timeout"}`)
- IPC inbound event delivery:
  - connected clients receive inbound broadcast events
  - per-client delivery uses bounded queues; lagging clients are disconnected instead of silently dropped
- Global verbosity override:
  - `--verbose` / `-v` sets default logging to `debug` (otherwise default is `info`)
  - if `RUST_LOG` is explicitly set, it takes precedence over `--verbose`
- Request payload shape:
  - `axon request` always sends payload as `{"message":"<string>"}` (including when the string itself is JSON text)
  - for fully structured request payload objects, use IPC `send` directly as documented in `spec/IPC.md`
- Doctor command behavior:
  - `axon doctor` runs local health checks and prints a human-readable checklist
  - `axon doctor --json` prints the structured report (`checks`, `fixes_applied`, `ok`)
  - `axon doctor --fix` applies safe local repairs; `--rekey` (requires `--fix`) allows identity reset when key data is unrecoverable (including non-base64/legacy raw `identity.key` contents)
  - returns exit code `2` when unresolved check failures remain (`ok: false`)

### Example interaction

```sh
axon examples    # prints a full annotated example interaction
```

## Message Types

| Kind | Stream | Purpose |
|------|--------|---------|
| `request` | Bidirectional | Send a request, get a `response` or `error` |
| `response` | Bidirectional | Reply to a request |
| `message` | Unidirectional | Fire-and-forget |
| `error` | Bidirectional or Unidirectional | Error reply or unsolicited error |

See [`spec/MESSAGE_TYPES.md`](./spec/MESSAGE_TYPES.md) for message kinds and stream mapping, and [`spec/WIRE_FORMAT.md`](./spec/WIRE_FORMAT.md) for the normative wire format.

## Configuration Reference

All settings are optional. AXON uses sensible defaults; you only need `config.yaml` to configure static peers or override defaults.

### `config.yaml`

Located at `~/.axon/config.yaml` by default (or `<state_root>/config.yaml` when overridden).

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `name` | `String` | _(none)_ | Optional display name for this agent. |
| `port` | `u16` | `7100` | QUIC listen port. CLI `--port` overrides this. |
| `advertise_addr` | `String` | _(none)_ | Optional `host:port` override used by `axon identity` URI output. |

#### Static peers

```yaml
name: alice
port: 7100
advertise_addr: "alice.tailnet:7100" # optional
peers:
  - agent_id: "ed25519.<hex>"
    addr: "10.0.0.5:7100"            # IP:port
    pubkey: "<base64-encoded-ed25519-public-key>"
  - agent_id: "ed25519.<hex>"
    addr: "my-peer.example.net:7100" # hostname:port
    pubkey: "<base64-encoded-ed25519-public-key>"
```

Hostname peers are resolved at startup/config load time (IPv4 preferred). Unresolvable peers are skipped with warning logs.

### Internal constants

These are compile-time constants and cannot be changed via configuration.

| Constant | Value | Location | Description |
|----------|-------|----------|-------------|
| `MAX_MESSAGE_SIZE` | `65536` (64 KB) | `message/envelope.rs` | Maximum encoded envelope size. Messages exceeding this are rejected. |
| `REQUEST_TIMEOUT` | `30s` | `transport/mod.rs` | Timeout for bidirectional request/response exchanges. |
| `STALE_TIMEOUT` | `60s` | `peer_table.rs` | Discovered (non-static, non-cached) peers with no activity for this duration are removed. |
| `MAX_IPC_LINE_LENGTH` | `64 KB` | `ipc/protocol.rs` | Maximum length of a single IPC command line. Overlong lines are rejected with `command_too_large`. |
| `MAX_CONNECTIONS` | `128` | `daemon/mod.rs` | Maximum simultaneous QUIC peer connections. |
| `KEEPALIVE` | `15s` | `daemon/mod.rs` | QUIC keepalive interval. |
| `IDLE_TIMEOUT` | `60s` | `daemon/mod.rs` | QUIC idle timeout. Connections with no traffic for this duration are closed. |
| `INBOUND_READ_TIMEOUT` | `10s` | `daemon/mod.rs` | Maximum time to wait for data on an inbound QUIC stream. |
| `MAX_IPC_CLIENTS` | `64` | `daemon/mod.rs` | Maximum simultaneous IPC client connections. |
| `MAX_CLIENT_QUEUE` | `1024` | `daemon/mod.rs` | Per-IPC-client outbound message queue depth; overflow disconnects lagging clients. |
| `RECONNECT_MAX_BACKOFF` | `30s` | `daemon/mod.rs` | Maximum backoff between reconnection attempts. Backoff starts at 1s and doubles. |
| Save interval | `60s` | `daemon/mod.rs` | How often the daemon persists `known_peers.json` to disk. |
| Stale cleanup interval | `5s` | `daemon/mod.rs` | How often the daemon checks for and removes stale discovered peers. |
| Reconnect interval | `1s` | `daemon/mod.rs` | How often the daemon checks for peers needing reconnection. |
| Initial reconnect backoff | `1s` | `daemon/reconnect.rs` | First reconnect attempt delay after a connection failure. Doubles up to `RECONNECT_MAX_BACKOFF`. |

## Documentation

| Document | Description |
|----------|-------------|
| [`spec/SPEC.md`](./spec/SPEC.md) | Protocol architecture — QUIC, Ed25519, discovery, lifecycle |
| [`spec/MESSAGE_TYPES.md`](./spec/MESSAGE_TYPES.md) | Message kinds and stream mapping |
| [`spec/WIRE_FORMAT.md`](./spec/WIRE_FORMAT.md) | Normative wire format for interoperable implementations |
| [`spec/IPC.md`](./spec/IPC.md) | IPC protocol — Unix socket commands |
| [`CONTRIBUTING.md`](./CONTRIBUTING.md) | Development guide, module map, testing requirements |
| [`rubrics/`](./rubrics/) | Evaluation rubrics — quality, documentation, alignment |
| [`SECURITY.md`](./SECURITY.md) | Security policy and vulnerability reporting |

## License

[MIT](./LICENSE)
