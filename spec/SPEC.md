# AXON Specification — QUIC Architecture

_Feb 14, 2026. Reference implementation in `axon/`._

## Overview

AXON is a lightweight background daemon that enables secure, fast, point-to-point messaging between agents on a local network. Each agent's machine runs one daemon.

```
OpenClaw ←→ [Unix Socket] ←→ AXON Daemon ←→ [QUIC/UDP] ←→ AXON Daemon ←→ [Unix Socket] ←→ OpenClaw
```

## Design Principles

1. **Point-to-point, not broadcast.** This is direct messaging between known peers. No pub/sub, no multicast, no fan-out.
2. **Zero-config on LAN.** Agents discover each other automatically. No IP addresses to configure for the common case.
3. **Secure by default.** All traffic encrypted with forward secrecy. Agents authenticate cryptographically.
4. **Lightweight.** <5MB RSS, negligible CPU when idle. Runs indefinitely.
5. **Expandable.** LAN-first, but the architecture must not preclude NAT traversal later. Discovery is pluggable; transport is NAT-friendly.

## 1. Identity

### Key Generation
- On first run, generate an **Ed25519** signing keypair.
- Store private key at `~/.axon/identity.key` (chmod 600).
- Store public key at `~/.axon/identity.pub` (base64).
- **Agent ID** = `ed25519.` prefix + first 16 bytes of SHA-256(public key), hex-encoded. 40 chars total (e.g. `ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4`). The type prefix enables future algorithm agility.

### Self-Signed Certificate
- On startup, generate a self-signed X.509 certificate from the Ed25519 keypair using `rcgen`.
- Certificate is ephemeral (regenerated each launch) — only the underlying keypair is persistent.
- This certificate is used for QUIC's TLS 1.3 handshake.

### Why Ed25519?
- Signing + identity in one keypair (no separate encryption keys needed — QUIC handles encryption).
- Fast signature generation/verification.
- Small keys (32 bytes public, 64 bytes private).
- Well-supported in Rust (`ed25519-dalek`, `rcgen`).

## 2. Discovery

### Primary: mDNS/DNS-SD (LAN, zero-config)
- Service type: `_axon._udp.local`
- TXT records: `agent_id=<hex>`, `pubkey=<base64 Ed25519 public key>`
- Browse continuously for peers; maintain a peer table.
- Stale peer removal: 60s without mDNS refresh.
- Re-advertise on startup and periodically.

### Fallback: Static Peers (config file)
```toml
# ~/.axon/config.toml
[[peers]]
agent_id = "ed25519.a1b2c3d4..."
addr = "100.64.0.5:7100"     # Tailscale IP
pubkey = "base64..."

[[peers]]
agent_id = "ed25519.e5f6a7b8..."
addr = "192.168.1.50:7100"   # LAN IP
pubkey = "base64..."
```
- Static peers are always in the peer table; mDNS-discovered peers are added/removed dynamically.
- Static config enables Tailscale/VPN use immediately without any protocol changes.

### Future: Rendezvous Server
- Not in v0.2, but the discovery layer should be a trait so we can add rendezvous + STUN later without touching transport or IPC.

### Discovery Trait (internal abstraction)
```rust
#[async_trait]
trait Discovery: Send + Sync {
    /// Start discovering peers. Push updates to the channel.
    async fn run(&self, tx: mpsc::Sender<PeerEvent>) -> Result<()>;
}

enum PeerEvent {
    Discovered { agent_id: AgentId, addr: SocketAddr, pubkey: Ed25519PublicKey },
    Lost { agent_id: AgentId },
}
```
Two implementations for v0.2: `MdnsDiscovery` and `StaticDiscovery`. Both feed the same peer table.

## 3. Transport: QUIC

### Why QUIC?
- **Encryption built-in:** TLS 1.3 with forward secrecy. No hand-rolled crypto.
- **Multiplexed streams:** Multiple concurrent messages without head-of-line blocking.
- **0-RTT reconnection:** Previously-connected peers can send data immediately.
- **Connection migration:** Survives IP changes (useful for mobile agents, DHCP renewal).
- **NAT-friendly:** UDP-based, connection IDs survive NAT rebinding. Future-proofs for internet use.

### Crate: `quinn`

### Connection Lifecycle
1. Peer discovered (via mDNS or static config).
2. **Lower agent_id initiates connection** (deterministic, prevents duplicate connections).
3. QUIC handshake includes TLS with self-signed cert.
4. **Receiver validates:** peer's certificate public key must match the agent_id/pubkey from discovery. Reject if mismatch (prevents MITM).
5. Connection stays open. Keepalive: QUIC idle timeout 60s with keepalive pings at 15s.
6. On disconnect: reconnect with exponential backoff (1s, 2s, 4s, ... max 30s). 0-RTT on reconnect.

### Stream Mapping
- **Bidirectional streams** for request/response pairs: `hello↔hello`, `ping↔pong`, `query↔response`, `delegate↔ack`, `cancel↔ack`, `discover↔capabilities`. Write request, read response, close. Timeout: 30s default (configurable per message).
- **Unidirectional streams** for fire-and-forget: `notify`, `result` (async task completion), unsolicited `error`.
- Stream contains: JSON bytes, delimited by QUIC stream FIN (no length prefix).
- Max message size: 64KB.
- No HOL blocking — each message gets its own stream.

### Listening
- Default port: 7100 (configurable via `--port` or config.toml).
- Bind to `0.0.0.0:7100` (accept from any interface).

## 4. Message Format

### Wire Format: JSON
- Rationale: LLMs produce and consume JSON natively. Our messages are <1KB. Parsing overhead is <0.1ms, dwarfed by network latency. Interoperability with any language/tool that speaks JSON.
- If profiling shows JSON is a bottleneck (unlikely), swap to msgpack — same serde derives, drop-in replacement.

### Envelope
```json
{
  "v": 1,
  "id": "uuid-v4",
  "from": "ed25519.a1b2c3d4...",
  "to": "ed25519.e5f6a7b8...",
  "ts": 1771108000000,
  "kind": "<message kind>",
  "ref": null,
  "payload": { ... }
}
```

- `v`: protocol version (negotiated via hello).
- `id`: unique message identifier (UUID v4).
- `from` / `to`: agent IDs (typed, e.g. `ed25519.` + first 16 bytes of SHA-256 of public key, hex, 40 chars).
- `ts`: unix milliseconds.
- `kind`: message type string. See `message-types.md` for the full set.
- `ref`: the message ID this responds to. Null for initiating messages.
- `payload`: kind-specific data. Unknown fields MUST be ignored (forward compatibility).

### Payload Kinds

**query** — Ask another agent a question.
```json
{
  "kind": "query",
  "question": "What are the kids' swim schedules this week?",
  "domain": "family.calendar",
  "max_tokens": 200,
  "deadline_ms": 30000
}
```
- `question`: natural language or structured query.
- `domain`: dot-separated topic hint. Optional. Helps receiver scope its answer.
- `max_tokens`: numeric budget for the response. 0 = no limit. Receiver should treat as a guideline.
- `deadline_ms`: how long sender waits before timing out. Optional, default 30000.

**response** — Answer to a query.
```json
{
  "kind": "response",
  "data": { ... },
  "summary": "Three swim practices: Mon/Wed/Fri 4-5pm"
}
```

**delegate** — Ask another agent to perform a task.
```json
{
  "kind": "delegate",
  "task": "Send a message to the family chat about dinner plans",
  "context": { "dinner_time": "7pm" },
  "priority": "normal",
  "report_back": true
}
```

**notify** — Inform without expecting a response.
```json
{
  "kind": "notify",
  "topic": "user.location",
  "data": { "status": "heading out", "eta_back": "2h" },
  "importance": "low"
}
```
- `importance`: `low` (background), `medium`, or `high` (act on this). Default `low`.

## 5. Local IPC: Unix Domain Socket

The full IPC specification is in [`spec/IPC.md`](./IPC.md). This section provides a summary.

### Socket Path
- `~/.axon/axon.sock`
- Removed on startup (clean stale sockets). Created fresh. Permissions: mode `0600`.

### Protocol
Line-delimited JSON over Unix socket. Each line is one complete JSON object. Protocol versioning via optional `hello` handshake (see `IPC.md` §1.2).

### Core Commands (v1)
```json
{"cmd": "send", "to": "<agent_id>", "kind": "query", "payload": { ... }}
{"cmd": "peers"}
{"cmd": "status"}
```

### v2 Commands
- **`hello`** — Protocol version negotiation, feature discovery, and consumer registration.
- **`auth`** — Token-based authentication (fallback when peer credentials unavailable).
- **`whoami`** — Daemon identity (agent_id, public key, version).
- **`inbox`** — Fetch buffered inbound messages (poll pattern).
- **`ack`** — Advance consumer acknowledgement cursor.
- **`subscribe`** — Stream inbound messages with optional replay and kind filtering.

IPC v2 commands include a `req_id` echoed in responses so clients can correlate responses while `subscribe` events interleave. See `IPC.md` §1.3.

### Receive Buffer
The IPC layer maintains a bounded in-memory receive buffer (default: 1000 messages, 24h TTL) so that messages arriving between client connections are not lost. Buffered messages have a daemon-assigned monotonic `seq`. IPC v2 inbox/ack state is tracked per `consumer` (from `hello`), and `ack` advances `acked_seq` for that consumer. See `IPC.md` §4.

### Authentication
Peer credential verification (`SO_PEERCRED`/`getpeereid`) as primary mechanism; token file as fallback. v1 clients (no `hello`) are exempt. See `IPC.md` §2.

### Multiple IPC Clients
- Multiple clients can connect to the socket simultaneously.
- v1 clients receive inbound messages via broadcast (legacy behavior).
- v2 clients receive inbound messages only via `subscribe` or `inbox`.
- Commands are handled by whichever client sends them.

## 6. CLI

```
axon daemon [--port 7100] [--agent-id <override>]
    Start the daemon. Runs in foreground (use systemd/launchd for background).

axon send <agent_id> <message>
    Send a quick query (convenience wrapper).

axon delegate <agent_id> <task>
    Delegate a task.

axon notify <agent_id> <topic> <data>
    Send a notification.

axon peers
    List discovered and connected peers with RTT.

axon status
    Daemon health: uptime, connections, message counts.

axon identity
    Print this agent's ID and public key.
```

All CLI commands (except `daemon`) connect to the Unix socket, send a command, print the response, and exit.

## 7. File Layout

```
~/.axon/
├── identity.key        # Ed25519 private key (chmod 600)
├── identity.pub        # Ed25519 public key (base64)
├── config.toml         # Optional: port, static peers
├── known_peers.json    # Cache of last-seen peer addresses (auto-managed)
└── axon.sock           # Unix domain socket (runtime only)
```

## 8. Daemon Lifecycle

### Startup
1. Load or generate identity keypair.
2. Generate ephemeral self-signed X.509 cert from keypair.
3. Read config.toml (if exists) for port and static peers.
4. Load known_peers.json cache.
5. Start QUIC endpoint (bind port).
6. Start mDNS advertisement + browsing.
7. Start Unix socket listener.
8. Initiate connections to known/discovered peers (lower ID initiates).

### Runtime
- Accept inbound QUIC connections (validate peer certs).
- Accept inbound IPC connections.
- Route messages: IPC → QUIC (outbound), QUIC → IPC (inbound).
- Maintain peer table from mDNS events + static config.
- Periodically save known_peers.json (every 60s or on peer change).

### Shutdown (SIGTERM/SIGINT)
1. Stop accepting new connections.
2. Send QUIC close frames to all peers (graceful).
3. Close Unix socket.
4. Save known_peers.json.
5. Remove socket file.
6. Exit.

## 9. Error Handling

- **Peer unreachable:** Fail immediately, return error to IPC client. The calling agent can retry if it wants. AXON is a transport — peer-to-peer delivery has no store-and-forward semantics. (The IPC layer maintains a local receive buffer for inbound messages; see `IPC.md` §4.)
- **Invalid peer cert:** Reject connection, log warning.
- **Malformed message:** Drop, log warning. Don't crash.
- **IPC client disconnects:** Clean up, no effect on other clients or QUIC connections.

## 10. Security Considerations

- **Forward secrecy:** Provided by QUIC's TLS 1.3. Ephemeral key exchange per connection. Compromising the static Ed25519 key does NOT decrypt past sessions.
- **MITM on first discovery (TOFU):** mDNS is unauthenticated. First discovery trusts the pubkey advertised. Mitigations: (a) known_peers.json pins pubkeys after first contact, (b) static config with pre-shared pubkeys for high-security setups, (c) future: out-of-band verification (QR code, etc.).
- **0-RTT replay:** QUIC 0-RTT data can be replayed. Mitigation: message IDs (UUIDs) for deduplication at application layer. Notify messages are idempotent anyway.
- **Local IPC security:** Unix socket permissions (user-only). No authentication beyond filesystem ACLs.

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
toml = "0.8"
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
3. All messages encrypted with forward secrecy.
4. Clean reconnect after daemon restart (0-RTT when possible).
5. Daemon uses <5MB RSS memory.
6. Static peer config works for Tailscale/VPN without code changes.
7. `axon send` CLI delivers a message end-to-end.
8. Graceful shutdown: no data loss, clean QUIC close.

## Future Considerations

- **OpenClaw transport integration:** AXON could register as an OpenClaw transport so agents use `sessions_send` natively, with AXON as the backend. For now, the Unix socket API is the interface.
