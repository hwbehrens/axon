# AXON Specification — QUIC Architecture

Status: Normative

_Feb 14, 2026. Reference implementation in `axon/`. Updated after architecture simplification (Phases 1–10)._

## Overview

AXON is a lightweight background daemon that enables secure, fast, point-to-point messaging between agents on a local network. Each agent's machine runs one daemon.

```
OpenClaw ←→ [Unix Socket] ←→ AXON Daemon ←→ [QUIC/UDP] ←→ AXON Daemon ←→ [Unix Socket] ←→ OpenClaw
```

## Design Principles

1. **Point-to-point, not broadcast.** This is direct messaging between known peers. No pub/sub, no multicast, no fan-out.
2. **Zero-config discovery, intentional trust.** Agents find candidates automatically on a LAN, but a local agent or operator explicitly enrolls each peer before communication.
3. **Secure by default.** All traffic encrypted with forward secrecy. Agents authenticate cryptographically via mTLS.
4. **Lightweight.** <5MB RSS, negligible CPU when idle. Runs indefinitely.
5. **Simple.** Minimal protocol surface, one owner for each fact, bounded in-memory state, and small inspectable files instead of service dependencies.

## 1. Identity

### Key Generation
- On first run, generate an **Ed25519** signing keypair.
- Store private key seed at `~/.axon/identity.key` as base64 text encoding of 32 bytes (chmod 600).
- Store public key at `~/.axon/identity.pub` (base64).
- Implementations MUST reject non-base64 or non-UTF-8 `identity.key` contents; automatic in-place migration from legacy raw seed files is not supported.
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

## 2. Discovery, Enrollment, and Peer State

### Bonjour/zeroconf discovery

- AXON uses DNS-SD/mDNS service type `_axon._udp.local.` for local-link discovery.
- DNS-SD SRV data supplies the current hostname and QUIC port.
- TXT records contain `agent_id=ed25519.<32 hex chars>` and `pubkey=<base64 Ed25519 public key>`.
- Browsing is continuous. Each service instance/interface observation has its own lifetime; losing one observation MUST NOT erase another observation for the same peer.
- Before exposing an observation, AXON validates that `agent_id` is derived from `pubkey` exactly as specified in §1.
- Observations are untrusted candidates. Discovery MUST NOT add a key to the TLS pin set or initiate an authenticated connection.

### Intentional enrollment

A local client enrolls a candidate with `add_peer`, either by selecting a currently observed Agent ID or by supplying an `axon://` peer token. Enrollment is the only transition that makes an identity trusted. Once enrolled:

- the `(Agent ID, public key)` binding is immutable;
- the same key may acquire and lose locators without changing trust;
- a conflicting key is rejected and surfaced;
- replacing the identity key requires removing the old peer and enrolling the new Agent ID.

AXON v1 has no automatic-TOFU admission mode.

### Peer directory ownership

`PeerDirectory` is the sole logical owner of validated enrolled peers, configured locators, and live candidate observations. It derives:

- an immutable `Agent ID → public key` pinning snapshot for TLS;
- dial targets for enrolled peers;
- read-only peer/candidate views for IPC.

`ConnectionManager` owns connection state and QUIC resources; it does not own or mutate trust. Discovery adapters only produce observations and cannot mutate trusted state directly.

### Scope

Automatic discovery is local-link only. AXON does not define a rendezvous, STUN, NAT traversal, or WAN discovery protocol. Explicit peer tokens may contain DNS or VPN/Tailscale locators. Configured hostnames are retained as names and resolved on every new connection attempt so endpoint rotation does not affect identity trust.

## 3. Transport: QUIC

### Why QUIC?
- **Encryption built-in:** TLS 1.3 with forward secrecy. No hand-rolled crypto.
- **Multiplexed streams:** Multiple concurrent messages without head-of-line blocking.
- **Connection migration:** Survives IP changes (useful for mobile agents, DHCP renewal).
- **NAT-friendly:** UDP-based, connection IDs survive NAT rebinding. Future-proofs for internet use.

### Crate: `quinn`

### Connection Lifecycle
1. A peer is explicitly enrolled and has at least one current or configured locator.
2. Either side may initiate a QUIC connection. When simultaneous cross-dials race, the lexicographically lower Agent ID is the preferred initiator; this is a tie-breaker, not a restriction on dialing an empty slot.
3. mTLS handshake: both sides present self-signed certificates with ALPN token `axon/1`. Each side validates the peer certificate against the enrolled pin. Unknown or mismatched peers are rejected during TLS.
4. Exactly one authoritative connection slot exists per peer. A healthy incumbent wins within its generation and duplicates are closed. The lexicographically-lower-initiator preference is only a tie-breaker for simultaneous cross-dials: a preferred-direction candidate may replace a healthy incumbent solely while that incumbent's cross-dial race can still be in flight (within one dial timeout of its installation); afterwards the healthy incumbent wins regardless of direction.
5. Failure, an unhealthy transition, or a failed/timed-out exchange advances the generation. Retrying the exchange redials rather than reusing suspect state. A replacement must authenticate before it occupies the empty slot.
6. On disconnect: reconnect with exponential backoff (1s, 2s, 4s, ... max 30s). Outcomes and teardown from an older generation cannot mutate a newer slot.

### Authentication
Authentication is solely via mTLS. TLS verifiers read an immutable pinning snapshot derived from `PeerDirectory`. A peer must be explicitly enrolled before a connection is accepted; discovery alone is insufficient. Unknown peers are rejected at the TLS layer, though their validated identity may be surfaced as an untrusted candidate for local approval.

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
  "kind": "request|response|message|error|describe",
  "payload": { ... },
  "ref": "uuid-v4-or-omitted"
}
```

- `id`: unique message identifier (UUID v4).
- `kind`: one of `request`, `response`, `message`, `error`, `describe`. Unknown kinds are preserved for forward compatibility.
- `payload`: arbitrary JSON object. No typed payload schemas — contents are application-defined. Unknown fields MUST be ignored (forward compatibility). The single exception is `describe`, whose response payload is the capability manifest (schema: spec/WIRE_FORMAT.md §6.5).
- `ref`: the message ID this responds to. Omitted for initiating messages.

Note: `from` and `to` are **not** on the wire. The daemon populates these fields for IPC clients based on the QUIC connection context.

### Message Kinds

- **`request`** — Ask another agent something. Expects a `response` or `error` reply on the same bidirectional stream.
- **`response`** — Reply to a `request`.
- **`message`** — Fire-and-forget notification. Sent on a unidirectional stream.
- **`error`** — Error reply to a `request` on a bidirectional stream, or unsolicited error on a unidirectional stream.
- **`describe`** — Capability-manifest query. Answered by the receiving daemon from the manifest its handler published at `serve` time; the application handler is never woken. Manifests are self-reported claims and never affect trust state. With no manifest published, the daemon replies `error`/`no_manifest` — explicit absence, never silence.

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
{"cmd": "add_peer", "agent_id": "<observed-candidate-id>"}
{"cmd": "add_peer", "token": "axon://<peer-token>"}
{"cmd": "remove_peer", "agent_id": "<agent_id>"}
{"cmd": "who_can", "query": "cargo"}
{"cmd": "serve", "manifest": { ... }}
{"cmd": "reply", "request_id": "<uuid>", "kind": "response|error", "payload": { ... }}
```

- **`send`** — Send a message to a remote peer over IPC. Requires `to`, `kind` (`request`, `message`, or `describe`), and `payload`. Optional `timeout_secs` applies to `request` and `describe`. `describe` is answered by the receiving daemon from its registered manifest.
- **`peers`** — List bounded candidate and enrolled peer views; connected peers include an advisory `services` summary when a manifest has been observed.
- **`status`** — Daemon health: uptime, connections, message counts.
- **`whoami`** — Daemon identity and metadata (`ok`, `agent_id`, `public_key`, optional `name`, `version`, `uptime_secs`).
- **`add_peer`** — Enroll a currently observed candidate or a peer token and persist it atomically.
- **`remove_peer`** — Revoke an enrolled peer, cancel its attempts, close its connection, and remove its durable record.
- **`who_can`** — Query which connected enrolled peers expose a matching service (cached `describe` pulls; advisory; `unreachable` names peers that failed a pull).
- **`serve`** — Acquire the daemon's single connection-bound inbound request-handler lease, optionally publishing a capability manifest that the daemon serves for `describe` requests.
- **`reply`** — Resolve one pending inbound request on its originating QUIC stream.

### Authentication
Unix socket permissions (`0600`, user-only) as baseline. Peer UID credential check (`SO_PEERCRED`/`getpeereid`) verifies connecting processes belong to the same user. No token-based auth.

### Multiple IPC Clients
- Multiple clients can connect to the socket simultaneously.
- All connected clients receive ordinary inbound messages via broadcast while they keep up with delivery.
- Per-client outbound IPC queues are bounded; a lagging client is disconnected on overflow rather than silently skipped.
- Requests are delivered only to the client holding the handler lease. A competing `serve` is rejected until the holder disconnects.
- The daemon-owned request broker admits at most one reply per pending request. No handler, handler loss, timeout, duplicate reply, late reply, and bounded-delivery failure are explicit terminal outcomes.
- AXON does not promise exactly-once application execution or durable request processing.

## 6. CLI

```
axon [-q | -v | -vv] [--state-root <dir>] daemon [--port 7100] [--disable-mdns]
    Start the daemon. Runs in foreground (use systemd/launchd for background).
    --disable-mdns uses enrolled peers' configured locators only.
    --state-root sets the AXON state root (socket/identity/config), enabling multi-agent-per-host layouts.
    Aliases: --state, --root. Env fallback: AXON_ROOT. Default: ~/.axon.
    Verbosity: -q (warn), default (info), -v (debug), -vv (trace).
    RUST_LOG takes precedence over verbosity flags when set.

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
    List discovered candidates and enrolled peers with trust and connection state.
    Human-readable table by default.

axon [--state-root <dir>] status [--json]
    Daemon health: uptime, connections, message counts.
    Human-readable key/value output by default.

axon [--state-root <dir>] identity
    Print this agent's share URI (`axon://...`) with a human-readable label by default.
    Use `--json` for full details (`agent_id`, `public_key`, `addr`, `port`, `uri`).
    Use `--addr host:port` to override the emitted URI address.
    This command is local/offline; it reads/writes identity files in the selected state root.

axon [--state-root <dir>] connect <axon://token-or-candidate-agent-id>
    Intentionally enroll a peer through the running daemon and persist it in peers.json.

axon [--state-root <dir>] forget <agent_id>
    Revoke an enrolled peer through the running daemon.

axon [--state-root <dir>] whoami [--json]
    Query daemon identity and metadata over IPC.
    Human-readable labeled output by default.

axon [--state-root <dir>] doctor [--json] [--fix] [--rekey]
    Diagnose local AXON state (identity, config, IPC socket, peer-store hygiene).
    Reports unsupported legacy peer state and invalid peers.json contents.
    Defaults to check mode. `--fix` applies safe repairs (with timestamped backups),
    and `--rekey` regenerates identity material when paired with `--fix`.
    Human-readable checklist output by default.

axon [--state-root <dir>] config <KEY> [VALUE]
axon [--state-root <dir>] config --list [--json]
axon [--state-root <dir>] config --unset <KEY>
axon [--state-root <dir>] config --edit
    Read/write local daemon settings: `name`, `port`, `advertise_addr`.
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
├── config.yaml         # Optional local daemon settings
├── peers.json          # Canonical enrolled-peer store (daemon-managed)
├── daemon.pid          # Runtime single-instance lock (removed on clean shutdown)
└── axon.sock           # Unix domain socket (runtime only)
```

### Config Format
```yaml
name: my-agent                         # optional display name
port: 7100                             # optional, default 7100
advertise_addr: "my-host.tail:7100"    # optional `axon identity` output override
```

Only `name`, `port`, and `advertise_addr` are configurable. Enrolled peers are not configuration; they are managed through IPC and stored in `peers.json`. All tuning values (timeouts, buffer sizes, intervals) are hardcoded as constants.

### Peer Store Format

`peers.json` is a small, versioned JSON document:

```json
{
  "version": 1,
  "peers": [
    {
      "agent_id": "ed25519.abc...",
      "pubkey": "<base64>",
      "locators": ["peer-host-or-ip:7100"]
    }
  ]
}
```

The store is bounded to 256 enrolled peers and 8 configured locators per peer. Entries are emitted in Agent ID order. Every load validates the entire document, Agent ID/public-key binding, locator syntax, uniqueness, and bounds; invalid state fails closed rather than partially loading. Writes use a temporary file in the same directory, file sync, atomic rename, and parent-directory sync. The daemon is the sole runtime writer.

`config.yaml` peer entries and `known_peers.json` are unsupported legacy state. AXON reports them with re-enrollment guidance and does not import or silently ignore them.

## 8. Daemon Lifecycle

### Startup
1. Load or generate identity keypair.
2. Generate ephemeral self-signed X.509 cert from keypair.
3. Read config.yaml (if exists) for port, name, and advertise_addr; reject legacy peer entries.
4. Load and validate peers.json into PeerDirectory; reject unsupported known_peers.json state.
5. Start Unix socket listener; clean it up if a later startup step fails.
6. Start QUIC endpoint (bind port).
7. Start mDNS advertisement + browsing.
8. Initiate connections only to enrolled peers with current dial targets.

### Runtime
- Accept inbound QUIC connections (mTLS validates peer certs against the current enrolled-pin snapshot).
- Accept inbound IPC connections.
- Route messages: IPC → QUIC (outbound), QUIC messages → IPC observers, and QUIC requests → the single IPC handler.
- Maintain candidate observations and enrolled peer state through PeerDirectory.
- Persist enrollment changes immediately through the atomic peer-store adapter. Ephemeral discovery and connection state are never persisted.

### Reconnection
On disconnect, reconnect attempts run as async tasks with in-flight deduplication (only one reconnect attempt per peer at a time). Exponential backoff: 1s initial, 30s max.

### Shutdown (SIGTERM/SIGINT)
1. Stop accepting new connections.
2. Cancel, close, and join every owned connection/attempt task.
3. Close Unix socket.
4. Remove socket file.
5. Exit.

## 9. Error Handling

- **Peer unreachable:** Fail immediately, return error to IPC client. The calling agent can retry if it wants. AXON is a transport — peer-to-peer delivery has no store-and-forward semantics.
- **Invalid peer cert:** Reject connection, log warning.
- **Malformed message:** Drop, log warning. Don't crash.
- **IPC client disconnects:** Clean up, no effect on other clients or QUIC connections.

## 10. Security Considerations

- **Forward secrecy:** Provided by QUIC's TLS 1.3. Ephemeral key exchange per connection. Compromising the static Ed25519 key does NOT decrypt past sessions.
- **Unauthenticated discovery:** mDNS advertisements are untrusted candidates. A same-user local IPC action must explicitly enroll a candidate or peer token before its key enters the TLS pin set. Discovery cannot replace an enrolled pin.
- **mTLS authentication:** Both sides of every QUIC connection present certificates. The peer's certificate public key must match an enrolled pin derived from PeerDirectory. Unknown peers are rejected at the TLS layer.
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
2. A local agent can inspect and intentionally enroll a discovered candidate without entering an IP address.
3. All messages encrypted with forward secrecy via mTLS.
4. Clean reconnect after daemon restart.
5. Daemon uses <5MB RSS memory.
6. Explicit peer tokens with DNS or Tailscale/VPN locators reconnect across address rotation.
7. `axon request` CLI delivers a message end-to-end.
8. Graceful shutdown: no data loss, clean QUIC close.

## Future Considerations

- **OpenClaw transport integration:** AXON could register as an OpenClaw transport so agents use `sessions_send` natively, with AXON as the backend. For now, the Unix socket API is the interface.
