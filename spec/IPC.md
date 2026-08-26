# AXON IPC Specification

Status: Normative

**Status:** Normative  
**Authors:** Kit (OpenClaw agent), Hans Behrens

---

## 1. Overview

The IPC interface connects same-user local clients (CLI tools and agent applications) to one AXON daemon over a Unix domain socket. It provides outbound messaging, candidate inspection, intentional peer enrollment and revocation, daemon status, identity queries, and one connection-bound inbound request handler.

IPC is transport plumbing. `PeerDirectory` owns peer trust and observations, while `RequestBroker` owns handler and pending-request state. IPC connections do not independently own either fact.

Ordinary inbound messages are broadcast to connected clients with deliver-or-disconnect backpressure. Inbound requests are delivered only to the one client holding the handler lease.

---

## 2. Socket, Framing, and Security

**Socket path:** `~/.axon/axon.sock` (or `<state_root>/axon.sock`).

**Protocol:** Line-delimited JSON. Each command, response, or event is one complete JSON object terminated by `\n`. Literal newlines inside strings MUST be escaped. There is no handshake or version negotiation.

**Maximum line size:** 65,536 bytes including the trailing newline. An overlong command receives `command_too_large` when possible and the connection is closed.

**Permissions:** The daemon creates the socket with mode `0600`.

**Peer credential check:** On connect, the daemon extracts the process UID using `SO_PEERCRED` (Linux) or `getpeereid` (macOS). A UID different from the socket owner's UID is rejected.

---

## 3. Common Command Rules

Every command is a JSON object with a `cmd` string. An optional `req_id` string may be included on any command and is echoed in its response.

Unknown IPC command names are rejected immediately with `invalid_command`. They are not retained or replayed after upgrades.

---

## 4. Commands

### 4.1 `send`

Send a message to an enrolled peer.

```json
{"cmd":"send","to":"<agent_id>","kind":"request|message","payload":{},"timeout_secs":30,"ref":"<uuid-optional>"}
```

`timeout_secs` is optional and only valid for `request`. It bounds the WHOLE exchange (dial, stream open, frame write, and response read share one deadline; phases never receive independent budgets). Values above `3600` are rejected with `invalid_command`.

For `message`:

```json
{"ok":true,"msg_id":"<uuid>"}
```

For `request`, the peer's response is returned inline:

```json
{"ok":true,"msg_id":"<uuid>","response":{}}
```

### 4.2 `peers`

List bounded read-only views of discovered candidates and enrolled peers.

```json
{"cmd":"peers"}
```

```json
{
  "ok": true,
  "peers": [
    {
      "agent_id": "ed25519.a1b2...",
      "public_key": "<base64>",
      "trust": "candidate|enrolled",
      "locators": ["host-or-ip:7100"],
      "status": "discovered|disconnected|connecting|connected|backoff",
      "rtt_ms": 1.23,
      "display_name": "optional-name"
    }
  ]
}
```

`rtt_ms` and `display_name` are omitted when unavailable. Candidate entries always use `status=discovered`; they are not trusted and cannot be messaged until enrolled.

### 4.3 `status`

```json
{"cmd":"status"}
```

```json
{"ok":true,"uptime_secs":3600,"peers_connected":1,"messages_sent":42,"messages_received":38}
```

### 4.4 `whoami`

```json
{"cmd":"whoami"}
```

```json
{"ok":true,"agent_id":"ed25519.a1b2...","public_key":"<base64>","name":"agent-name","version":"0.8.0","uptime_secs":3600}
```

`name` is omitted when unset.

### 4.5 `add_peer`

Intentionally enroll either a currently observed candidate or an out-of-band peer token. Exactly one of `agent_id` and `token` MUST be present.

```json
{"cmd":"add_peer","agent_id":"ed25519.a1b2..."}
```

```json
{"cmd":"add_peer","token":"axon://<pubkey-base64url>@<host-or-ip>:7100"}
```

The daemon validates Agent ID/public-key binding and locator syntax, atomically persists the complete next peer-store snapshot, commits the PeerDirectory transition, and only then reports success:

```json
{"ok":true,"agent_id":"ed25519.a1b2..."}
```

Re-enrolling the same Agent ID and public key is idempotent. A conflicting key is rejected and never replaces the existing pin.

### 4.6 `remove_peer`

Revoke an enrolled peer.

```json
{"cmd":"remove_peer","agent_id":"ed25519.a1b2..."}
```

The daemon atomically persists the complete next peer-store snapshot, removes the pin and dial targets, cancels connection attempts, closes the active connection, and reports:

```json
{"ok":true,"agent_id":"ed25519.a1b2..."}
```

Removing a candidate observation is not supported; it expires with discovery.

### 4.7 `serve`

Acquire the daemon's single inbound request-handler lease for the lifetime of this IPC connection.

```json
{"cmd":"serve"}
```

```json
{"ok":true,"serving":true}
```

A second client receives `handler_busy`. Repeating `serve` on the lease holder is idempotent. The lease is released immediately when its IPC connection closes.

### 4.8 `reply`

Resolve one pending request delivered to the current handler.

```json
{"cmd":"reply","request_id":"<uuid>","peer":"<agent_id-optional>","kind":"response|error","payload":{}}
```

```json
{"ok":true,"request_id":"<uuid>"}
```

Only the handler that received the request may reply. Exactly one reply is admitted. Requests are correlated per authenticated remote peer: `request_id` alone can be ambiguous when two peers present the same UUID. Supplying `peer` (the `from` identity delivered with the request event) disambiguates; when it is omitted and the ID matches several pending requests, the reply is rejected with `invalid_command` rather than being routed to an arbitrary peer. Duplicate, late, unknown, and non-owner replies are rejected explicitly. A reply whose encoded envelope would exceed the 65,536-byte wire limit is rejected with `invalid_command` before the request is consumed; the handler may retry with a smaller payload.

---

## 5. Errors

```json
{"ok":false,"error":"<code>","message":"<explanation>","req_id":"<optional>"}
```

| Code | Condition |
|---|---|
| `invalid_command` | Malformed JSON, unknown command, or invalid/mutually exclusive fields. |
| `command_too_large` | IPC command exceeds 65,536 bytes. |
| `peer_not_found` | Target is not an enrolled peer. |
| `peer_not_observed` | Candidate enrollment names no current observation. |
| `peer_conflict` | Agent ID/public-key binding conflicts with enrolled state. |
| `self_send` | Target is the local Agent ID. |
| `peer_unreachable` | Peer is enrolled but no connection could be established. |
| `timeout` | Request timed out waiting for a peer response. |
| `handler_busy` | Another IPC connection owns the handler lease. |
| `not_handler` | The client attempted a handler-only operation without the lease. |
| `request_not_found` | Request is unknown, expired, disconnected, or already completed. |
| `send_capacity_exceeded` | The daemon's outbound send budget is exhausted; only `send` commands are rejected, control commands remain served. Retry shortly. |
| `internal_error` | Unexpected daemon or persistence failure. |

Errors are instructive and MUST NOT report a timeout as `peer_unreachable`.

---

## 6. Events

Events contain `event` and never contain `ok` or `req_id`.

### 6.1 Ordinary inbound message

Broadcast to connected clients:

```json
{"event":"inbound","from":"<agent_id>","envelope":{}}
```

Losslessly retained unknown unidirectional message kinds use the same event.

### 6.2 Inbound request

Delivered only to the handler lease holder:

```json
{"event":"request","request_id":"<request-uuid>","from":"<agent_id>","envelope":{}}
```

If no handler exists, handler delivery overflows, the handler disconnects, or the handler deadline expires, the broker sends one terminal `error` response to the remote requester. AXON guarantees at most one protocol reply, not exactly-once application execution.

### 6.3 Peer candidate

Broadcast when a new validated but untrusted candidate becomes observable:

```json
{"event":"peer_candidate","agent_id":"<agent_id>","public_key":"<base64>","locators":["host-or-ip:7100"],"source":"mdns|handshake"}
```

Candidate events are hints. Only `add_peer` establishes trust.

---

## 7. Multiple Clients and Backpressure

Up to 64 IPC clients may connect simultaneously. Each has a bounded outbound queue, and the request broker retains at most 1,024 pending inbound requests.

- Ordinary inbound messages and candidate events are broadcast to clients that keep up.
- Queue overflow disconnects the lagging client instead of silently dropping a subset.
- Inbound requests are queued only to the handler. Handler queue overflow terminates the lease and the affected request with an explicit remote error.
- Messages arriving with no observer are dropped; AXON has no durable inbox or store-and-forward behavior.

---

## 8. Multi-Agent Per Host

Each local agent identity runs a separate daemon with its own state root, Unix socket, QUIC port, identity, and peer store. The daemon does not multiplex identities.
