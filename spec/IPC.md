# AXON IPC Specification

**Version:** 2  
**Status:** Normative  
**Authors:** Kit (OpenClaw agent), Hans Behrens  

---

## 1. Overview

The IPC interface bridges local client processes (CLI tools, agents) with the AXON daemon over a Unix domain socket. It provides message sending, peer management, identity queries, and a **receive buffer** that preserves inbound messages between client connections.

### 1.1 Design Principles

1. **Zero overhead for v1 clients.** Clients using only `send`/`peers`/`status` MUST NOT pay for buffer or auth features.
2. **The daemon stays lightweight.** No HTTP client. No external dependencies beyond the QUIC transport.
3. **Existing commands are unchanged.** All v1 commands work identically.
4. **Boundary is the Unix socket.** QUIC transport semantics are unaffected. Peer-to-peer delivery is unchanged: if a peer is unreachable, `send` fails immediately.

### 1.2 Protocol Versioning and Handshake

IPC is line-delimited JSON (one complete JSON object per line).
IPC **v2+** adds an explicit `hello` handshake.

**Client → daemon (`hello` request):**
```json
{"cmd": "hello", "req_id": "1", "version": 2, "consumer": "default"}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `req_id` | string | Recommended | Opaque request identifier echoed in the response. Not required for `hello` since the protocol version is not yet negotiated. |
| `version` | integer | Yes | Highest IPC version the client supports. |
| `consumer` | string | No | Consumer name used for inbox/ack state. Default: `"default"`. Max 64 bytes UTF-8. |

**Daemon → client (`hello` response):**
```json
{"ok": true, "req_id": "1", "version": 2, "daemon_max_version": 2, "agent_id": "ed25519.a1b2c3d4...", "features": ["buffer", "subscribe", "auth"]}
```

| Field | Type | Description |
|-------|------|-------------|
| `version` | integer | **Negotiated** IPC version for this connection: `min(client.version, daemon_max_version)`. |
| `daemon_max_version` | integer | Maximum IPC version supported by the daemon. |
| `agent_id` | string | This daemon's agent identity. |
| `features` | array of string | Optional features available in this build. |

**Negotiation rule:** The daemon computes `version = min(client.version, daemon_max_version)` and returns it. Clients MUST NOT compute negotiation themselves; they MUST use the `version` field from the response.

After a successful `hello`, all semantics on that connection are governed by the negotiated `version`.

**v1 compatibility:** If a client does not send `hello`, the daemon MUST assume v1 semantics and accept all v1 commands without authentication. This ensures backward compatibility.

If a client skips `hello` and sends a v2-only command (`whoami`, `inbox`, `ack`, `subscribe`), the daemon MUST reject it with `hello_required`.

**Hardened mode:** When `ipc.allow_v1 = false`, the daemon MUST reject any connection that does not complete `hello` negotiating `version >= 2`. A `hello` that negotiates version 1 MUST be rejected with error code `unsupported_version`. See §8.

### 1.3 Request/Response Correlation (v2+)

For negotiated IPC `version >= 2`:
- Every client command MUST include `req_id` (string).
- Every command response MUST include the same `req_id`.
- Exception: `hello` MAY omit `req_id` since the protocol version is not yet negotiated when `hello` is sent. If `hello` includes `req_id`, the daemon MUST echo it. All commands after `hello` MUST include `req_id`.
- The daemon MAY send pushed inbound events at any time while a subscription is active (see §3.5), including between a request and its response.

A pushed inbound event is identified by the presence of `"event": "inbound"` (see §3.5). It never carries `ok` or `req_id`.

Clients MUST demultiplex by:
- If the object contains `"event": "inbound"`, treat it as a pushed event.
- Otherwise, treat it as a response and correlate by `req_id`.

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

**Token file creation requirements (normative):**
- The daemon MUST create the token file with mode `0600`.
- The daemon MUST write the token atomically (write to a temporary file then `rename`) OR open with `O_CREAT|O_EXCL` to avoid races.
- The daemon MUST NOT follow symlinks when creating or writing the token file.
- If the existing token file is not a regular file owned by the daemon UID, the daemon MUST refuse to use it and MUST log an error.

### 2.3 `auth` Command

**Request:**
```json
{"cmd": "auth", "req_id": "2", "token": "<hex-encoded-256-bit-token>"}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `req_id` | string | Yes (v2+) | Response correlation. |
| `token` | string (hex) | Yes | 64-character hex-encoded token from `~/.axon/ipc-token`. |

**Response (success):**
```json
{"ok": true, "req_id": "2", "auth": "accepted"}
```

**Response (failure):**
```json
{"ok": false, "req_id": "2", "error": "auth_failed"}
```

When peer credentials already authenticated the connection, `auth` is accepted but unnecessary.

### 2.4 Hardening Configuration

By default, the daemon accepts v1 clients without authentication for backward compatibility. Deployments requiring stronger local security MAY set `ipc.allow_v1 = false`:

| Config Key | Default | Effect |
|---|---|---|
| `ipc.allow_v1` | `true` | When `false`, the daemon rejects IPC v1 behavior and requires `hello` negotiating `version >= 2` for all connections. |

When `ipc.allow_v1 = false`, any command received before a successful `hello` MUST be rejected with `hello_required`, and a `hello` that negotiates version 1 MUST be rejected with `unsupported_version`.

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
{"cmd": "whoami", "req_id": "3"}
```

**Response:**
```json
{
  "ok": true,
  "req_id": "3",
  "agent_id": "ed25519.a1b2c3d4e5f6a7b8...",
  "public_key": "<base64-encoded-Ed25519-public-key>",
  "name": "<human-readable-name-from-config>",
  "version": "0.1.0",
  "ipc_version": 2,
  "uptime_secs": 3600
}
```

### 3.3 `inbox`

Fetch buffered inbound messages for the connection's `consumer`.

**Request:**
```json
{"cmd": "inbox", "req_id": "4", "limit": 50, "kinds": ["query", "delegate"]}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `req_id` | string | Yes | — | Response correlation (see §1.3). |
| `limit` | integer | No | 50 | Maximum messages to return. Range: 1–1000. |
| `kinds` | array of string | No | *(all kinds)* | Filter by envelope `kind`. Unknown kinds MUST cause `invalid_command`. |

**Response:**
```json
{
  "ok": true,
  "req_id": "4",
  "messages": [
    {"seq": 101, "buffered_at_ms": 1771108200123, "envelope": {}},
    {"seq": 102, "buffered_at_ms": 1771108260456, "envelope": {}}
  ],
  "next_seq": 102,
  "has_more": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `messages` | array | Buffered envelopes with receipt metadata. Ordered oldest-first. Each message includes daemon-assigned `seq`. |
| `next_seq` | integer \| null | The highest `seq` returned in this response, or `null` if no messages were returned. |
| `has_more` | boolean | `true` if more messages are available beyond `limit`. |

**Selection rule:** `inbox` returns messages with `seq > consumer.acked_seq` that match `kinds`.

When the buffer is empty or all messages have been acknowledged, the response is `{"ok": true, "req_id": "4", "messages": [], "next_seq": null, "has_more": false}`.

### 3.4 `ack`

Advance the consumer's acknowledgement cursor. This marks messages as processed for that consumer.

**Request:**
```json
{"cmd": "ack", "req_id": "5", "up_to_seq": 102}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `req_id` | string | Yes | Response correlation. |
| `up_to_seq` | integer | Yes | Acknowledge all messages with `seq <= up_to_seq` for this consumer. |

**Rules:**
- The daemon MUST only accept `up_to_seq` values that were previously delivered to this consumer via `inbox` or `subscribe`.
- If `up_to_seq` is greater than the highest delivered sequence for this consumer, the daemon MUST respond with `ack_out_of_range`.

**Response:**
```json
{"ok": true, "req_id": "5", "acked_seq": 102}
```

### 3.5 `subscribe`

Opens a streaming subscription on the current connection for the connection's `consumer`.

**Request:**
```json
{"cmd": "subscribe", "req_id": "6", "kinds": ["query", "delegate", "notify"], "replay": true}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `req_id` | string | Yes | — | Response correlation. |
| `kinds` | array of string | No | *(all kinds)* | Filter by envelope `kind`. Unknown kinds MUST cause `invalid_command`. |
| `replay` | boolean | No | `true` | If `true`, the daemon first replays buffered messages with `seq > consumer.acked_seq` that match `kinds`, then switches to live delivery. |

**Immediate response:**
```json
{"ok": true, "req_id": "6", "subscribed": true, "replayed": 3, "replay_to_seq": 105}
```

**Pushed inbound event (replay or live):**
```json
{"event": "inbound", "replay": false, "seq": 106, "buffered_at_ms": 1771108300123, "envelope": {}}
```

**Replay snapshot rule (normative):** When `subscribe` is received, the daemon defines a snapshot `replay_to_seq` equal to the current highest buffered `seq`. If `replay = true`, it MUST emit replay events (with `"replay": true`) in increasing `seq` order up to `replay_to_seq`, then begin emitting live events (with `"replay": false`) for newly buffered messages (`seq > replay_to_seq`). No message may be emitted twice on the same subscription.

A connection MAY have at most one active subscription. Sending `subscribe` again MUST replace the previous filter.

**Delivery rules by connection type:**

| Connection type | Inbound delivery |
|---|---|
| **v1** (no `hello`) | Legacy broadcast: all inbound messages pushed to all connected v1 clients. |
| **v2, no subscription** | No unsolicited inbound messages. Client MUST use `inbox` (pull) to retrieve buffered messages. |
| **v2, active subscription** | Inbound messages matching the subscription filter are pushed as `{"event": "inbound", ...}` lines. Non-matching messages are buffered only. |

v1 broadcast and v2 subscriptions are separate delivery paths. A v2 client never receives legacy broadcast messages.

### 3.6 Stream Interleaving (v2+)

While a subscription is active, the daemon MAY interleave pushed inbound events with command responses on the same connection.

Determinism rules:
1. Each JSON object is newline-delimited and MUST be sent atomically (no splitting a JSON object across multiple lines).
2. Every response object MUST contain `req_id`.
3. Pushed inbound events MUST contain `"event": "inbound"` and MUST NOT contain `ok` or `req_id`.

Clients MUST demultiplex by:
- If the object contains `"event": "inbound"`, treat it as a pushed event.
- Otherwise, treat it as a response and correlate by `req_id`.

---

## 4. Receive Buffer

### 4.1 Purpose

The receive buffer preserves inbound messages that arrive when no IPC client is connected (or when connected clients have not subscribed to the relevant message kinds). This is an IPC-layer concern — QUIC transport semantics are unchanged.

### 4.2 Semantics

- **Storage:** In-memory `VecDeque`.
- **Ordering:** Each buffered message is assigned a monotonically increasing `seq: u64` by the daemon. Sequence numbers are unique within a daemon process lifetime and never reused. They reset on daemon restart.
- **Capacity:** Configurable via `ipc.buffer_size` in `config.toml`. Default: **1000**. When `buffer_size = 0`, no messages are buffered and `inbox` always returns empty.
- **Byte bound:** The daemon enforces an internal hard byte cap (default: 4 MB) to keep memory bounded regardless of `buffer_size`. When total buffered bytes exceed this limit, the oldest messages are evicted until under the limit.
- **TTL:** Configurable via `ipc.buffer_ttl_secs`. Default: 86400 (24 hours). Expired messages are evicted on the next `inbox` call or buffer append.
- **Eviction:** When the buffer is full (by count or bytes), the oldest message is dropped (FIFO). No per-kind bucketing.
- **Delivery interaction:** Inbound messages are delivered to subscribed/broadcast clients AND appended to the buffer. Messages persist until TTL expiry or eviction; `ack` advances per-consumer cursors but does not delete messages from the buffer (other consumers may not have processed them yet).

### 4.3 Consumer Semantics (normative)

Inbox/ack state is scoped by `consumer` (from `hello`).

- Each consumer has `acked_seq` (initially `0`).
- `inbox` and `subscribe` (with `replay = true`) deliver messages with `seq > acked_seq` that match the requested `kinds`.
- `ack(up_to_seq)` advances `acked_seq` for that consumer only.
- The daemon MAY garbage-collect inactive consumer state after an implementation-defined period. If consumer state is garbage-collected, its `acked_seq` resets to `0`.
- **Single-consumer recommendation:** For simple deployments, use the default consumer name (`"default"`) from a single process. Multiple consumers are supported but require each process to use a distinct consumer name to avoid cursor interference.

---

## 5. Error Codes

All error responses use the format `{"ok": false, "req_id": "<id>", "error": "<code>"}`. Additional fields MAY be present. For v1 connections, `req_id` is omitted.

| Code | HTTP-like | Condition |
|------|-----------|-----------|
| `hello_required` | 400 | v2 command sent without prior `hello` handshake. |
| `unsupported_version` | 400 | `hello` negotiated a version the daemon cannot accept (e.g., v1 when `allow_v1 = false`). |
| `auth_required` | 401 | Command requires authentication (v2+ connection, token mode). |
| `auth_failed` | 403 | Invalid token or unauthorized UID. |
| `invalid_command` | 400 | Malformed JSON, unknown `cmd`, missing required field, or invalid field value (e.g., unknown message kind in `kinds` filter). |
| `ack_out_of_range` | 400 | `up_to_seq` exceeds the highest sequence delivered to this consumer. |
| `peer_not_found` | 404 | Target agent_id not in peer table. |
| `peer_unreachable` | 502 | Peer known but QUIC connection failed or timed out. |
| `internal_error` | 500 | Unexpected daemon error. |

---

## 6. Multi-Agent Per Host

When multiple agents share a host, each MUST run its own daemon instance with:

- Separate socket path (e.g., `~/.axon/<agent-name>/axon.sock`)
- Separate QUIC port
- Separate identity (keypair)
- Separate receive buffer

The daemon does NOT multiplex between agents. One daemon = one identity.

> **Token path:** When running multiple agents per host, each daemon's `ipc.token_path` MUST point to its own instance directory (e.g., `~/.axon/<agent-name>/ipc-token`). The default `~/.axon/ipc-token` will collide if multiple daemons share the same home directory.

---

## 7. Backward Compatibility

| Client behavior | Daemon response (default) | With `allow_v1 = false` |
|----------------|--------------------------|------------------------|
| Skips `hello`, sends v1 commands directly | Accepted. Legacy broadcast. No auth required. | Rejected with `hello_required`. |
| Skips `hello`, sends v2 command | Rejected with `hello_required`. | Rejected with `hello_required`. |
| Sends `hello` with `version: 2` | Negotiated version 2. Auth required (token mode) or implicit (peer credentials). | Same. |
| Sends `hello` with `version: 1` | Negotiated version 1. v1 semantics apply (broadcast, no auth required). | Rejected with `unsupported_version`. |
| Sends unknown `cmd` | `{"ok": false, "error": "invalid_command"}` | Same. |
| v2 client connects to v1 daemon | `hello` returns `invalid_command`. Client SHOULD fall back to v1 behavior. | N/A (v1 daemon). |

---

## 8. Configuration

New `config.toml` fields under `[ipc]`:

```toml
[ipc]
# Compatibility
allow_v1 = true             # Accept v1 (no-hello) connections

# Receive buffer
buffer_size = 1000          # Max buffered messages (0 = disabled)
buffer_ttl_secs = 86400     # Message TTL in seconds (24 hours)
buffer_byte_cap = 4194304   # Hard byte cap on buffer memory (4 MB)

# Auth (token mode fallback)
token_path = "~/.axon/ipc-token"  # Token file location
```

All fields are optional with the defaults shown.
