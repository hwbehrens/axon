# AXON IPC Specification

**Status:** Normative  
**Authors:** Kit (OpenClaw agent), Hans Behrens  

---

## 1. Overview

The IPC interface connects local client processes (CLI tools, agents) with the AXON daemon over a Unix domain socket. It provides message sending, peer listing, daemon status, and identity queries via a simple line-delimited JSON protocol.

All inbound messages from peers are broadcast to connected IPC clients (deliver-or-disconnect under bounded-queue backpressure).

---

## 2. Socket and Security

**Socket path:** `~/.axon/axon.sock` (Unix domain socket).

**Protocol:** Line-delimited JSON — one complete JSON object per `\n`-terminated line. No handshake or version negotiation.

**Permissions:** The daemon creates the socket with mode `0600` (owner read/write only).

**Peer credential check:** On connect, the daemon extracts the connecting process's UID via `SO_PEERCRED` (Linux) or `getpeereid` (macOS). If the UID does not match the socket-owning UID, the connection is rejected.

---

## 3. Commands

All commands are JSON objects with a `"cmd"` field. An optional `"req_id"` (string) may be included on any command; if present, the daemon echoes it in the response.

### 3.1 `send`

Send a message to a peer.

**Request:**
```json
{"cmd": "send", "to": "<agent_id>", "kind": "request|message", "payload": {...}, "ref": "<uuid-optional>"}
```

**Response (unidirectional):**
```json
{"ok": true, "msg_id": "<uuid>"}
```

**Response (bidirectional — inline response from peer):**
```json
{"ok": true, "msg_id": "<uuid>", "response": {...}}
```

### 3.2 `peers`

List connected peers.

**Request:**
```json
{"cmd": "peers"}
```

**Response:**
```json
{"ok": true, "peers": [{"agent_id": "<agent_id>", "addr": "ip:port", "status": "connected", "rtt_ms": 1.23, "source": "static"}]}
```

`agent_id` is the canonical peer identity field name in `peers` responses.

### 3.3 `status`

Daemon status.

**Request:**
```json
{"cmd": "status"}
```

**Response:**
```json
{"ok": true, "uptime_secs": 3600, "peers_connected": 1, "messages_sent": 42, "messages_received": 38}
```

### 3.4 `whoami`

Daemon identity.

**Request:**
```json
{"cmd": "whoami"}
```

**Response:**
```json
{"ok": true, "agent_id": "ed25519.a1b2...", "public_key": "<base64>", "name": "agent-name", "version": "0.5.0", "uptime_secs": 3600}
```

Response shape notes:
- `public_key` is standard base64 (Ed25519 public key).
- `name` is optional and may be omitted when unset.

---

## 4. Error Codes

All error responses use the format:
```json
{"ok": false, "error": "<code>", "message": "<explanation>"}
```

If `req_id` was present on the command, it is echoed in the error response.

| Code | Condition |
|------|-----------|
| `invalid_command` | Malformed JSON, unknown `cmd`, or missing/invalid field. |
| `command_too_large` | IPC commands over 64 KB are rejected. |
| `peer_not_found` | Target `agent_id` not in peer table. |
| `self_send` | Sending to your own `agent_id` is rejected. |
| `peer_unreachable` | Peer known but QUIC connection failed or timed out. |
| `internal_error` | Unexpected daemon error. |

---

## 5. Inbound Events

All inbound messages from peers are broadcast to connected IPC clients as unsolicited events:

```json
{"event": "inbound", "from": "<agent_id>", "envelope": {...}}
```

Inbound events are identified by the presence of `"event": "inbound"`. They never carry `"ok"` or `"req_id"`.

Clients demultiplex by checking for `"event": "inbound"` — if present, it is a pushed event; otherwise it is a command response.

The daemon uses a bounded per-client outbound queue. If a client cannot keep up and its queue overflows, that client is disconnected (deliver-or-disconnect semantics). Messages arriving when no IPC client is connected are dropped.

---

## 6. Multiple Clients

Up to 64 IPC clients may connect simultaneously. Connected clients that keep up receive all inbound broadcast events. Lagging clients are disconnected on queue overflow. Commands are handled independently per client.

---

## 7. Multi-Agent Per Host

When multiple agents share a host, each MUST run its own daemon instance with:

- Separate socket path (e.g., `~/.axon/<agent-name>/axon.sock`)
- Separate QUIC port
- Separate identity (keypair)

The daemon does NOT multiplex between agents. One daemon = one identity.
