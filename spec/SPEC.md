# AXON Specification — QUIC Architecture

_Feb 14, 2026. Reference implementation in `axon/`. Updated after architecture simplification (Phases 1–10)._

## Overview

AXON is a lightweight background daemon that enables secure, fast, point-to-point messaging between agents on a local network. Each agent's machine runs one daemon.

```
OpenClaw ←→ [Unix Socket] ←→ AXON Daemon ←→ [QUIC/UDP] ←→ AXON Daemon ←→ [Unix Socket] ←→ OpenClaw
```

## Design Principles

1. **Point-to-point, not broadcast.** This is direct messaging between known peers. No pub/sub, no multicast, no fan-out.
2. **Zero-config on LAN.** Agents discover each other automatically. No IP addresses to configure for the common case.
3. **Secure by default.** All traffic encrypted with forward secrecy. Agents authenticate cryptographically via mTLS.
4. **Lightweight.** <5MB RSS, negligible CPU when idle. Runs indefinitely.
5. **Simple.** Minimal protocol surface. Four message kinds, five IPC commands. No unnecessary abstraction layers.

## 1. Identity

### Key Generation
- On first run, generate an **Ed25519** signing keypair.
- Store private key seed at `~/.axon/identity.key` as base64 text encoding of 32 bytes (chmod 600).
- Store public key at `~/.axon/identity.pub` (base64).
- **Agent ID** = `ed25519.` prefix + first 16 bytes of SHA-256(public key), hex-encoded. 40 chars total (e.g. `ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4`). The type prefix enables future algorithm agility.

### Self-Signed Certificate
- On startup, generate a self-signed X.509 certificate from the Ed25519 keypair using `rcgen`.
- Certificate is ephemeral (regenerated each launch) — only the underlying keypair is persistent.
- This certificate is used for QUIC's TLS 1.3 handshake (mTLS).

### Why Ed25519?
- Signing + identity in one keypair (no separate encryption keys needed — QUIC handles encryption).
- Fast signature generation/verification.
- Small keys (32 bytes public, 64 bytes private).
- Well-supported in Rust (`ed25519-dalek`, `rcgen`).

## 2. Discovery

### Primary: mDNS/DNS-SD (LAN, zero-config)
- Service type: `_axon._udp.local.`
- TXT records: `agent_id=ed25519.<32 hex chars>`, `pubkey=<base64 Ed25519 public key>`
- Browse continuously for peers; maintain a peer table.
- Stale peer removal: 60s without mDNS refresh.
- Re-advertise on startup and periodically.

### Fallback: Static Peers (config file)
```yaml
# ~/.axon/config.yaml
port: 7100
advertise_addr: "my-laptop.tail1234.ts.net:7100" # optional override for `axon identity`
peers:
  - agent_id: "ed25519.a1b2c3d4..."
    addr: "100.64.0.5:7100"                    # IP:port
    pubkey: "base64..."
  - agent_id: "ed25519.e5f6a7b8..."
    addr: "my-laptop.tail1234.ts.net:7100"     # hostname:port
    pubkey: "base64..."
```
- Static peers are always in the peer table; mDNS-discovered peers are added/removed dynamically.
- Static config enables Tailscale/VPN use immediately without any protocol changes.
- Hostnames are resolved at config-load time (IPv4 preferred). Unresolvable peers are skipped with warning logs.

### Implementation
Discovery is implemented as plain async functions (`run_mdns_discovery`, `run_static_discovery`) that push `PeerEvent`s to a channel. Both feed the same peer table.

```rust
enum PeerEvent {
    Discovered { agent_id: AgentId, addr: SocketAddr, pubkey: Ed25519PublicKey },
    Lost { agent_id: AgentId },
}
```

### Future: Rendezvous Server
- Not in v0.2, but the discovery functions can be extended with additional implementations (e.g., rendezvous + STUN) without touching transport or IPC.

## 3. Transport: QUIC

### Why QUIC?
- **Encryption built-in:** TLS 1.3 with forward secrecy. No hand-rolled crypto.
- **Multiplexed streams:** Multiple concurrent messages without head-of-line blocking.
- **Connection migration:** Survives IP changes (useful for mobile agents, DHCP renewal).
- **NAT-friendly:** UDP-based, connection IDs survive NAT rebinding. Future-proofs for internet use.

### Crate: `quinn`

### Connection Lifecycle
1. Peer discovered (via mDNS or static config).
2. Either side can initiate a QUIC connection (no deterministic initiator rule).
3. mTLS handshake: both sides present self-signed certificates with ALPN token `axon/1`. Each side validates the peer's certificate public key against the expected pubkey from discovery. Reject if mismatch (prevents MITM) or ALPN negotiation fails.
4. Connection stays open. Keepalive pings at 15s, idle timeout 60s.
5. On disconnect: reconnect with exponential backoff (1s, 2s, 4s, ... max 30s).

### Authentication
Authentication is solely via mTLS. The `PeerTable` owns a shared `PubkeyMap` that TLS certificate verifiers read directly. A peer must be discovered (mDNS or static config) before a connection is accepted — unknown peers are rejected at the TLS layer.

### Stream Mapping
| Kind | Stream | Purpose |
|------|--------|---------|
| `request` | Bidirectional | Send a request, expect a response |
| `response` | Bidirectional | Reply to a request |
| `message` | Unidirectional | Fire-and-forget |
| `error` | Bidirectional (reply) or Unidirectional (unsolicited) | Error reply to a request, or unsolicited error |

- Stream contains: JSON bytes, delimited by QUIC stream FIN (no length prefix).
- Max message size: 64KB.
- No HOL blocking — each message gets its own stream.

### Listening
- Default port: 7100 (configurable via `--port` or config.yaml).
- Bind to `0.0.0.0:7100` (accept from any interface).

## 4. Message Format

### Wire Format: JSON
- Rationale: LLMs produce and consume JSON natively. Our messages are <1KB. Parsing overhead is <0.1ms, dwarfed by network latency. Interoperability with any language/tool that speaks JSON.
- If profiling shows JSON is a bottleneck (unlikely), swap to msgpack — same serde derives, drop-in replacement.

### Wire Envelope
```json
{
  "id": "uuid-v4",
  "kind": "request|response|message|error",
  "payload": { ... },
  "ref": "uuid-v4-or-omitted"
}
```

- `id`: unique message identifier (UUID v4).
- `kind`: one of `request`, `response`, `message`, `error`. Unknown kinds are preserved for forward compatibility.
- `payload`: arbitrary JSON object. No typed payload schemas — contents are application-defined. Unknown fields MUST be ignored (forward compatibility).
- `ref`: the message ID this responds to. Omitted for initiating messages.

Note: `from` and `to` are **not** on the wire. The daemon populates these fields for IPC clients based on the QUIC connection context.

### Message Kinds

- **`request`** — Ask another agent something. Expects a `response` or `error` reply on the same bidirectional stream.
- **`response`** — Reply to a `request`.
- **`message`** — Fire-and-forget notification. Sent on a unidirectional stream.
- **`error`** — Error reply to a `request` on a bidirectional stream, or unsolicited error on a unidirectional stream.

## 5. Local IPC: Unix Domain Socket

### Socket Path
- `~/.axon/axon.sock`
- Removed on startup (clean stale sockets). Created fresh. Permissions: mode `0600`.

### Protocol
Line-delimited JSON over Unix socket. Each line is one complete JSON object. Single protocol — no version negotiation or handshake.

### Commands
```json
{"cmd": "send", "to": "<agent_id>", "kind": "request", "timeout_secs": 30, "payload": { ... }}
{"cmd": "peers"}
{"cmd": "status"}
{"cmd": "whoami"}
{"cmd": "add_peer", "pubkey": "<base64>", "addr": "host:port"}
```

- **`send`** — Send a message to a remote peer over IPC. Requires `to`, `kind` (`request` or `message`), and `payload`. Optional `timeout_secs` applies to `kind=request`.
- **`peers`** — List discovered and connected peers.
- **`status`** — Daemon health: uptime, connections, message counts.
- **`whoami`** — Daemon identity and metadata (`ok`, `agent_id`, `public_key`, optional `name`, `version`, `uptime_secs`).
- **`add_peer`** — Enroll a new static peer at runtime from `pubkey` + `addr`.

### Authentication
Unix socket permissions (`0600`, user-only) as baseline. Peer UID credential check (`SO_PEERCRED`/`getpeereid`) verifies connecting processes belong to the same user. No token-based auth.

### Multiple IPC Clients
- Multiple clients can connect to the socket simultaneously.
- All connected clients receive inbound messages via broadcast while they keep up with delivery.
- Per-client outbound IPC queues are bounded; a lagging client is disconnected on overflow rather than silently skipped.
- Commands are handled by whichever client sends them.

## 6. CLI

```
axon [--state-root <dir>] daemon [--port 7100] [--disable-mdns]
    Start the daemon. Runs in foreground (use systemd/launchd for background).
    --disable-mdns uses static peers only.
    --state-root sets the AXON state root (socket/identity/config), enabling multi-agent-per-host layouts.
    Aliases: --state, --root. Env fallback: AXON_ROOT. Default: ~/.axon.

axon [--state-root <dir>] request [--timeout <seconds>] <agent_id> <message>
    Send a request to a peer.
    For structured request payload objects, use IPC `send` directly.
    Exit code 2 when the remote returns an envelope with `kind=error`.
    Exit code 3 on request timeout.

axon [--state-root <dir>] notify [--json] <agent_id> <message>
    Send a fire-and-forget message to a peer.
    Default payload mode is literal text.
    `--json` parses the message as JSON and fails if invalid.

axon [--state-root <dir>] peers [--json]
    List discovered and connected peers with RTT.
    Human-readable table by default.

axon [--state-root <dir>] status [--json]
    Daemon health: uptime, connections, message counts.
    Human-readable key/value output by default.

axon [--state-root <dir>] identity
    Print this agent's share URI (`axon://...`) with a human-readable label by default.
    Use `--json` for full details (`agent_id`, `public_key`, `addr`, `port`, `uri`).
    Use `--addr host:port` to override the emitted URI address.
    This command is local/offline; it reads/writes identity files in the selected state root.

axon [--state-root <dir>] connect <axon://token>
    Enroll a peer from token into config.yaml and hot-load it into a running daemon via IPC.

axon [--state-root <dir>] whoami [--json]
    Query daemon identity and metadata over IPC.
    Human-readable labeled output by default.

axon [--state-root <dir>] doctor [--json] [--fix] [--rekey]
    Diagnose local AXON state (identity, config, IPC socket).
    Defaults to check mode. `--fix` applies safe repairs, and `--rekey` regenerates identity material when paired with `--fix`.
    Human-readable checklist output by default.

axon [--state-root <dir>] config <KEY> [VALUE]
axon [--state-root <dir>] config --list [--json]
axon [--state-root <dir>] config --unset <KEY>
axon [--state-root <dir>] config --edit
    Read/write scalar config keys: `name`, `port`, `advertise_addr`.
    Follows git-style config conventions (get/set/list/unset/edit).

axon [--state-root <dir>] examples
    Print example usage.

axon --version
axon -V
    Print CLI version.
```

CLI execution contracts:
- `request`/`notify`/`peers`/`status`/`whoami` use IPC.
- `peers`/`status`/`whoami` default to human-readable output; `--json` prints daemon JSON.
- `identity` and `doctor` are local and do not use IPC (`doctor --json` available).
- Exit code `0`: success.
- Exit code `1`: local/runtime failure after argument parsing (I/O, socket connect, decode).
- Exit code `2`: CLI parse/usage failure (Clap), daemon/application-level failure (`{"ok":false}` reply), or `request` remote envelope with `kind=error`.
- Exit code `3`: `request` timeout (`{"ok":false,"error":"timeout"}`).

## 7. File Layout

```
~/.axon/
├── identity.key        # Ed25519 private seed (base64 text, chmod 600)
├── identity.pub        # Ed25519 public key (base64)
├── config.yaml         # Optional: name, port, advertise_addr, static peers
├── known_peers.json    # Cache of last-seen peer addresses (auto-managed)
└── axon.sock           # Unix domain socket (runtime only)
```

### Config Format
```yaml
name: my-agent                         # optional display name
port: 7100                             # optional, default 7100
advertise_addr: "my-host.tail:7100"    # optional `axon identity` output override
peers:
  - agent_id: "ed25519.abc..."
    addr: "10.0.0.2:7100"              # or "hostname:7100"
    pubkey: "base64..."
```

Only `name`, `port`, `advertise_addr`, and `peers` are configurable. All tuning values (timeouts, buffer sizes, intervals) are hardcoded as constants.

## 8. Daemon Lifecycle

### Startup
1. Load or generate identity keypair.
2. Generate ephemeral self-signed X.509 cert from keypair.
3. Read config.yaml (if exists) for port, name, advertise_addr, and static peers.
4. Load known_peers.json cache.
5. Start QUIC endpoint (bind port).
6. Start mDNS advertisement + browsing.
7. Start Unix socket listener.
8. Initiate connections to known/discovered peers.

### Runtime
- Accept inbound QUIC connections (mTLS validates peer certs against peer table).
- Accept inbound IPC connections.
- Route messages: IPC → QUIC (outbound), QUIC → IPC (inbound, broadcast to connected clients; lagging IPC clients are disconnected when their bounded queue overflows).
- Maintain peer table from mDNS events + static config.
- Periodically save known_peers.json (every 60s or on peer change).

### Reconnection
On disconnect, reconnect attempts run as async tasks with in-flight deduplication (only one reconnect attempt per peer at a time). Exponential backoff: 1s initial, 30s max.

### Shutdown (SIGTERM/SIGINT)
1. Stop accepting new connections.
2. Send QUIC close frames to all peers (graceful).
3. Close Unix socket.
4. Save known_peers.json.
5. Remove socket file.
6. Exit.

## 9. Error Handling

- **Peer unreachable:** Fail immediately, return error to IPC client. The calling agent can retry if it wants. AXON is a transport — peer-to-peer delivery has no store-and-forward semantics.
- **Invalid peer cert:** Reject connection, log warning.
- **Malformed message:** Drop, log warning. Don't crash.
- **IPC client disconnects:** Clean up, no effect on other clients or QUIC connections.

## 10. Security Considerations

- **Forward secrecy:** Provided by QUIC's TLS 1.3. Ephemeral key exchange per connection. Compromising the static Ed25519 key does NOT decrypt past sessions.
- **MITM on first discovery (TOFU):** mDNS is unauthenticated. First discovery trusts the pubkey advertised. Mitigations: (a) known_peers.json pins pubkeys after first contact, (b) static config with pre-shared pubkeys for high-security setups, (c) future: out-of-band verification (QR code, etc.).
- **mTLS authentication:** Both sides of every QUIC connection present certificates. The peer's certificate public key must match a known pubkey from the peer table. Unknown peers are rejected at the TLS layer.
- **Local IPC security:** Unix socket permissions (`0600`, user-only) as baseline. Peer UID credential check (`SO_PEERCRED`/`getpeereid`) ensures only the owning user can connect.

## 11. Dependencies

See `axon/Cargo.toml` for current pinned versions. The versions below are indicative:

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
quinn = "0.10"
rustls = { version = "0.21", features = ["dangerous_configuration"] }  # Custom cert validation
rcgen = "0.11"
ed25519-dalek = "2"
mdns-sd = "0.11"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
clap = { version = "4", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
sha2 = "0.10"
base64 = "0.22"
uuid = { version = "1", features = ["v4"] }
anyhow = "1"
```

## 12. Success Criteria

1. Two daemons on the same LAN discover each other within 5 seconds.
2. Point-to-point message delivery in <10ms on LAN.
3. All messages encrypted with forward secrecy via mTLS.
4. Clean reconnect after daemon restart.
5. Daemon uses <5MB RSS memory.
6. Static peer config works for Tailscale/VPN without code changes.
7. `axon request` CLI delivers a message end-to-end.
8. Graceful shutdown: no data loss, clean QUIC close.

## Future Considerations

- **OpenClaw transport integration:** AXON could register as an OpenClaw transport so agents use `sessions_send` natively, with AXON as the backend. For now, the Unix socket API is the interface.
