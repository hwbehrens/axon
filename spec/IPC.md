# AXON IPC Specification

**Version:** 2  
**Status:** Normative  
**Authors:** Kit (OpenClaw agent), Hans Behrens  

---

## 1. Overview

The IPC interface bridges local client processes (CLI tools, agents) with the AXON daemon over a Unix domain socket. It provides message sending, peer management, identity queries, and a **receive buffer** that preserves inbound messages between client connections.

### 1.1 Design Principles

1. **Zero overhead when unused.** Clients using only `send`/`peers`/`status` MUST NOT pay for buffer or auth features.
2. **The daemon stays lightweight.** No HTTP client. No external dependencies beyond the QUIC transport.
3. **Existing commands are unchanged.** All v1 commands work identically.
4. **Boundary is the Unix socket.** QUIC transport semantics are unaffected. Peer-to-peer delivery is unchanged: if a peer is unreachable, `send` fails immediately.

### 1.2 Protocol Versioning

On connection, the client SHOULD send a `hello` command as its first message:

```json
{"cmd": "hello", "version": 2}
```

**Response:**
```json
{"ok": true, "version": 2, "agent_id": "ed25519.a1b2c3d4...", "features": ["auth", "buffer", "subscribe"]}
```

| Field | Type | Description |
|-------|------|-------------|
| `version` | integer | Highest IPC protocol version the daemon supports. |
| `agent_id` | string | This daemon's agent identity. |
| `features` | array of string | Optional features available in this build. |

If a client skips `hello`, the daemon MUST assume v1 semantics and accept all v1 commands without authentication. This ensures backward compatibility.

If a client skips `hello` and sends a v2-only command (`whoami`, `inbox`, `ack`, `subscribe`), the daemon MUST reject it with `hello_required`. These commands are only available after a successful `hello` handshake.

If a client sends `hello` with a `version` higher than the daemon supports, the daemon MUST respond with its own highest supported version. The **negotiated version** for the connection is `min(client_requested, daemon_supported)`. All subsequent semantics on that connection are governed by the negotiated version. If the negotiated version is 1, v2 rules (auth gating, subscription-only delivery) do not apply.

---

## 2. Authentication

Authentication is OPTIONAL for v1-compatible clients (those that skip `hello` or negotiate version 1). When `hello` negotiates `version >= 2`, authentication is REQUIRED before any command other than `hello`, `auth`, and `status`.

> **Security note:** Baseline local security is provided by filesystem permissions — the socket is `0600`, so only the owning UID can connect. Peer credential auth is primarily a defense-in-depth check and an enabler for hardened configurations (e.g., group-accessible sockets or future TCP localhost transport). Token auth provides the same role as a cross-platform fallback.

### 2.1 Peer Credentials (Primary)

On Linux (`SO_PEERCRED`) and macOS (`LOCAL_PEERCRED`/`getpeereid`), the daemon MUST extract the connecting process's UID from the socket on accept. If the UID matches the socket-owning UID, the connection is implicitly authenticated. No `auth` command is needed.

### 2.2 Token File (Fallback)

When peer credential extraction is unavailable or fails, the daemon falls back to token-based authentication:

1. On startup, the daemon generates a random 256-bit token and writes it to `~/.axon/ipc-token` (mode `0600`).
2. The client MUST send `auth` after `hello`. Until authenticated, commands other than `hello`, `auth`, and `status` return error code `auth_required`.
3. Token rotation is a filesystem operation (regenerate file, `SIGHUP` daemon). It is NOT an IPC command.

### 2.3 `auth` Command

**Request:**
```json
{"cmd": "auth", "token": "<hex-encoded-256-bit-token>"}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `token` | string (hex) | Yes | 64-character hex-encoded token from `~/.axon/ipc-token`. |

**Response (success):**
```json
{"ok": true, "auth": "accepted"}
```

**Response (failure):**
```json
{"ok": false, "error": "auth_failed"}
```

When peer credentials already authenticated the connection, `auth` is accepted but unnecessary.

### 2.4 Hardening Configuration

By default, the daemon accepts v1 clients without authentication for backward compatibility. Deployments requiring stronger local security MAY enable:

| Config Key | Default | Effect |
|---|---|---|
| `ipc.require_hello` | `false` | When `true`, reject all commands except `hello` until handshake completes. Disables v1 unauthenticated access. |
| `ipc.require_auth` | `false` | When `true`, require authentication for ALL connections (including v1). Useful when socket ACLs allow more than one UID. |

When `require_hello = true`, the backward compatibility table in §7 changes: "skips hello" rows become rejections.

> **Note on `status` before auth:** On v2 connections in token mode, `status` is accessible without authentication. This may expose operational details (uptime, peer count, buffer count). Deployments where this is a concern SHOULD use `require_auth = true` or restrict socket access via filesystem permissions.

---

## 3. Commands

### 3.1 Existing Commands (v1)

These commands are unchanged from v1. See `spec/SPEC.md` §5 for their original definitions.

- **`send`** — Send a message to a peer.
- **`peers`** — List connected peers with status and RTT.
- **`status`** — Daemon uptime, peer count, message counters.

### 3.2 `whoami`

Returns this daemon's identity.

**Request:**
```json
{"cmd": "whoami"}
```

**Response:**
```json
{
  "ok": true,
  "agent_id": "ed25519.a1b2c3d4e5f6a7b8...",
  "public_key": "<base64-encoded-Ed25519-public-key>",
  "name": "<human-readable-name-from-config>",
  "version": "0.1.0",
  "ipc_version": 2,
  "uptime_secs": 3600
}
```

### 3.3 `inbox`

Fetch messages from the receive buffer.

**Request:**
```json
{
  "cmd": "inbox",
  "limit": 50,
  "since": "2026-02-15T08:00:00Z",
  "kinds": ["query", "delegate"]
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `limit` | integer | No | 50 | Maximum messages to return. Range: 1–1000. |
| `since` | string | No | *(all buffered)* | Cursor for pagination. If the value is a valid UUID, return messages buffered **after** the message with that ID (in buffer order). If not a UUID, parse as ISO 8601 timestamp and return messages buffered **after** that time. If the UUID is not found in the buffer, return all messages (same as omitting `since`). |
| `kinds` | array of string | No | *(all kinds)* | Filter by message kind. Unknown kinds MUST be silently ignored. |

**Response:**
```json
{
  "ok": true,
  "messages": [
    {"envelope": { }, "buffered_at": "2026-02-15T08:30:00.123Z"},
    {"envelope": { }, "buffered_at": "2026-02-15T08:31:00.456Z"}
  ],
  "has_more": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `messages` | array | Buffered envelopes with receipt timestamp. Ordered oldest-first. |
| `has_more` | boolean | `true` if more messages exist beyond `limit`. |

When the buffer is empty, the response is `{"ok": true, "messages": [], "has_more": false}`.

### 3.4 `ack`

Acknowledge processed messages. The daemon removes them from the receive buffer.

**Request:**
```json
{"cmd": "ack", "ids": ["uuid-1", "uuid-2"]}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `ids` | array of UUID | Yes | Message IDs (envelope `id` field) to acknowledge. Unknown IDs MUST be silently ignored. |

**Response:**
```json
{"ok": true, "acked": 2}
```

### 3.5 `subscribe`

Opens a streaming subscription on the current connection. The daemon pushes matching inbound messages as they arrive.

**Request:**
```json
{
  "cmd": "subscribe",
  "since": "2026-02-15T08:00:00Z",
  "kinds": ["query", "delegate", "notify"]
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `since` | string | No | *(no replay)* | Replay buffered messages after this point before streaming live. |
| `kinds` | array of string | No | *(all kinds)* | Filter by message kind. |

**Response (immediate):**
```json
{"ok": true, "subscribed": true, "replayed": 3}
```

Followed by zero or more pushed messages:
```json
{"inbound": true, "envelope": { }}
```

A connection MAY have at most one active subscription. Sending `subscribe` again MUST replace the previous filter.

**Delivery rules by connection type:**

| Connection type | Inbound delivery |
|---|---|
| **v1** (no `hello`) | Legacy broadcast: all inbound messages pushed to all connected v1 clients. |
| **v2, no subscription** | No unsolicited inbound messages. Client MUST use `inbox` (pull) to retrieve buffered messages. |
| **v2, active subscription** | Inbound messages matching the subscription filter are pushed as `{"inbound": true, ...}` lines. Non-matching messages are buffered only. |

v1 broadcast and v2 subscriptions are separate delivery paths. A v2 client never receives legacy broadcast messages.

### 3.6 Stream Interleaving

A v2 connection with an active subscription carries both request/response traffic and pushed inbound messages on a single line-delimited stream. The following rule applies:

- While a subscription is active, the client SHOULD limit commands to `ack` and `subscribe` (to replace the filter). The daemon MAY interleave pushed `{"inbound": true, ...}` messages between any two response lines.
- Each response line is atomic (complete JSON object, newline-terminated) and MUST NOT be split by interleaved pushes.
- Clients that need concurrent request/response alongside streaming SHOULD open a second IPC connection for commands.

> **Rationale:** Adding `req_id` correlation would be more robust but adds complexity. For v2, the simple interleaving rule is sufficient given LLM agent usage patterns (subscribe on one connection, poll/send on another). A future v3 MAY introduce request IDs if needed.

---

## 4. Receive Buffer

### 4.1 Purpose

The receive buffer preserves inbound messages that arrive when no IPC client is connected (or when connected clients have not subscribed to the relevant message kinds). This is an IPC-layer concern — QUIC transport semantics are unchanged.

### 4.2 Semantics

- **Storage:** In-memory `VecDeque`. Optional disk persistence (§4.3).
- **Capacity:** Configurable via `ipc.buffer_size` in `config.toml`. Default: **0 (disabled)**. When `buffer_size = 0`, no messages are buffered and `inbox` always returns empty. Set to a positive value (e.g., 1000) to enable.
- **Byte bound:** Configurable via `ipc.buffer_max_bytes`. Default: 4194304 (4MB). When total buffered bytes exceed this limit, the oldest messages are evicted until under the limit. This ensures the buffer respects the daemon's <5MB RSS goal regardless of `buffer_size`.
- **TTL:** Configurable via `ipc.buffer_ttl_secs`. Default: 86400 (24 hours). Expired messages are evicted on the next `inbox` call or buffer append.
- **Eviction:** When the buffer is full (by count or bytes), the oldest message is dropped (FIFO). No per-kind bucketing.
- **Delivery interaction:** Inbound messages are delivered to subscribed/broadcast clients AND (when buffering is enabled) appended to the buffer. The `ack` command removes messages from the buffer. Unacked messages persist until TTL expiry or eviction.
- **Shared mailbox:** The receive buffer is a single shared mailbox for the daemon instance. Multiple connected clients share the same buffer. `ack` from any client removes messages for all clients. Multiple local processes consuming from the same daemon MUST coordinate externally to avoid stealing messages from each other. Clients SHOULD `ack` once they have durably processed a message to avoid unbounded replays.

> **Zero overhead when unused:** When `buffer_size = 0` (default), no allocation occurs for the receive buffer. Clients that only use `send`/`peers`/`status` pay no buffering cost.

### 4.3 Disk Persistence (Reserved, Non-Normative)

> **Status: Future.** This section is non-normative and describes a reserved design direction. It is NOT implemented in the current version. The `ipc.persist` configuration key is reserved. Implementers MUST NOT treat this section as required behavior.

When `ipc.persist = true` in `config.toml`:

- Inbound messages are written as individual JSON files to `~/.axon/inbox/<message-id>.json`.
- On startup, the daemon scans this directory to rebuild the buffer.
- `ack` deletes the corresponding file.
- Maximum disk usage is bounded by `buffer_size × max_message_size`.
- File format: the full `Envelope` JSON, one file per message.

> **Performance note:** One file per message incurs fsync cost and directory scan overhead on restart. This design is intentionally simple for low-volume use. High-volume deployments may require a different approach (embedded log, etc.).

When `ipc.persist = false` (default), the buffer is memory-only and lost on daemon restart.

---

## 5. Error Codes

All error responses use the format `{"ok": false, "error": "<code>"}`. Additional fields MAY be present.

| Code | HTTP-like | Condition |
|------|-----------|-----------|
| `hello_required` | 400 | v2 command sent without prior `hello` handshake. |
| `auth_required` | 401 | Command requires authentication (v2+ connection, token mode). |
| `auth_failed` | 403 | Invalid token or unauthorized UID. |
| `invalid_command` | 400 | Malformed JSON, unknown `cmd`, or missing required field. |
| `peer_not_found` | 404 | Target agent_id not in peer table. |
| `peer_unreachable` | 502 | Peer known but QUIC connection failed or timed out. |
| `rate_limited` | 429 | Client exceeded send rate limit. |
| `internal_error` | 500 | Unexpected daemon error. |

Rate limit response includes retry guidance:
```json
{"ok": false, "error": "rate_limited", "retry_after_ms": 1000}
```

### 5.1 Rate Limits (Reserved, Non-Normative)

> **Status: Future.** This section is non-normative and describes a reserved design direction. It is NOT implemented in the current version. The configuration keys and error code are reserved. Implementers MUST NOT treat this section as required behavior.

| Limit | Default | Configurable |
|-------|---------|-------------|
| Per-client send rate | 60 messages/minute | `ipc.rate_limit_per_client` |
| Global outbound rate | 300 messages/minute | `ipc.rate_limit_global` |
| Max concurrent IPC clients | 8 | `ipc.max_clients` |

Rate limits will use a sliding window. When a client exceeds the limit, subsequent `send` commands will return `rate_limited` with `retry_after_ms` indicating when the window resets.

---

## 6. Multi-Agent Per Host

When multiple agents share a host, each MUST run its own daemon instance with:

- Separate socket path (e.g., `~/.axon/<agent-name>/axon.sock`)
- Separate QUIC port
- Separate identity (keypair)
- Separate receive buffer and inbox directory

The daemon does NOT multiplex between agents. One daemon = one identity.

> **Token path:** When running multiple agents per host, each daemon's `ipc.token_path` MUST point to its own instance directory (e.g., `~/.axon/<agent-name>/ipc-token`). The default `~/.axon/ipc-token` will collide if multiple daemons share the same home directory.

---

## 7. Backward Compatibility

| Client behavior | Daemon response (default) | With `require_hello = true` |
|----------------|--------------------------|----------------------------|
| Skips `hello`, sends v1 commands directly | Accepted. Legacy broadcast. No auth required. | Rejected with `hello_required`. |
| Skips `hello`, sends v2 command | Rejected with `hello_required`. | Rejected with `hello_required`. |
| Sends `hello` with `version: 2` | Daemon responds with supported version. Auth required (token mode) or implicit (peer credentials). | Same. |
| Sends `hello` with `version: 1` | Negotiated version 1. v1 semantics apply (broadcast, no auth required). | Negotiated version 1. With `require_auth = true`, auth is still required. |
| Sends unknown `cmd` | `{"ok": false, "error": "invalid_command"}` | Same. |
| v2 client connects to v1 daemon | `hello` returns `invalid_command`. Client SHOULD fall back to v1 behavior. | N/A (v1 daemon). |

---

## 8. Configuration

New `config.toml` fields under `[ipc]`:

```toml
[ipc]
# Receive buffer
buffer_size = 0             # Max buffered messages (0 = disabled)
buffer_max_bytes = 4194304  # Max total buffered bytes (4MB)
buffer_ttl_secs = 86400     # Message TTL in seconds
persist = false             # Write buffer to disk (Future)

# Rate limits (Future)
rate_limit_per_client = 60  # Max sends per client per minute
rate_limit_global = 300     # Max total outbound per minute
max_clients = 8             # Max simultaneous IPC connections

# Auth
token_path = "~/.axon/ipc-token"  # Token file location (token mode only)
require_hello = false       # Reject commands before hello handshake
require_auth = false        # Require auth for all connections (including v1)
```

All fields are optional with the defaults shown.
