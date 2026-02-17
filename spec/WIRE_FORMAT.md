# AXON Wire Format Specification (Protocol v1)

_Status:_ Normative for AXON protocol version **1**.  
_Scope:_ This document specifies the **on-the-wire** formats and behaviors needed to build a compatible implementation in any language **without reading the Rust source**.

---

## 0. Conformance language

The key words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are to be interpreted as described in RFC 2119.

---

## 1. Protocol overview (wire-level)

AXON is a secure point-to-point messaging protocol over:

- **QUIC** over UDP (via QUIC streams)
- **TLS 1.3** authentication using **mutual TLS (mTLS)** with **Ed25519** keys
- **JSON** message encoding, delimited by QUIC stream FIN
- **One application message per QUIC stream** (both uni and bidi)

There are two interfaces:

1. **Network protocol:** QUIC/TLS/streams carrying framed JSON envelopes.
2. **Local IPC protocol:** Unix domain socket, **newline-delimited JSON** commands/replies.

---

## 2. Identities and cryptographic binding

### 2.1 Ed25519 key material

Each daemon has a long-lived Ed25519 signing keypair.

- Public key: 32 bytes (Ed25519)
- Private seed: 32 bytes (Ed25519 seed)

### 2.2 Agent ID derivation (normative)

**Agent ID** is a typed identifier derived from the Ed25519 public key bytes:

1. Compute `SHA-256(pubkey_bytes)` (32-byte digest).
2. Take the **first 16 bytes** of the digest.
3. Encode as **lowercase hex**, 2 hex chars per byte.
4. Prepend the algorithm type prefix: `ed25519.`.

Result: `ed25519.` followed by **32 ASCII hex characters** (total 40 characters).

**Example (shape):**
```
ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4   (length 40)
```

**Conformance:**
- Implementations **MUST** compute Agent ID exactly as above for Ed25519 keys.
- Implementations **MUST** include the `ed25519.` type prefix.
- The separator **MUST** be `.` (dot), not `:` (colon). The dot is used because Agent IDs are passed as TLS SNI server names, and colons are not valid in DNS labels.
- Implementations **MUST** treat the hex portion as case-insensitive for comparison, but **MUST** emit lowercase hex.
- The type prefix enables future algorithm agility. Implementations encountering an unknown type prefix **MUST** reject the peer.

### 2.3 QUIC/TLS certificate (normative properties)

Each daemon presents a (self-signed) X.509 certificate whose **SubjectPublicKeyInfo** contains the Ed25519 public key.

- The certificate is *ephemeral per process start* (regenerated), but the underlying Ed25519 keypair is persistent.
- The certificate **MUST** contain the Ed25519 public key that corresponds to the daemon's persistent identity key.

The reference implementation uses:
- X.509 CommonName: `axon-<agent_id>`
- SAN/DNS name includes `"localhost"` (not used for peer identity)
- Signature algorithm: Ed25519

Other implementations do **not** need to match CN/SAN values, but they **MUST**:
- Provide an X.509 cert containing the correct Ed25519 public key.
- Support mTLS and enforce the peer-validation rules below.

---

## 3. QUIC connection lifecycle (network protocol)

### 3.1 UDP port and bind

Default UDP port: **7100** (configurable).  
Server binds to `0.0.0.0:<port>` (all interfaces).

### 3.2 TLS usage and peer authentication (pinning)

AXON uses TLS 1.3 over QUIC with **mutual authentication**.

Either side may initiate a connection to any discovered peer. There is no deterministic initiator rule.

#### 3.2.1 SNI / "server name" usage (normative)

When initiating an outbound QUIC connection to a peer with Agent ID `REMOTE_ID`:

- The client **MUST** set the TLS Server Name (SNI) to the full typed Agent ID string (e.g. `ed25519.a1b2...`). The dot separator ensures the Agent ID is a valid DNS name for SNI purposes.

This SNI value is used as an *identity label* for certificate verification (not DNS).

#### 3.2.2 Server certificate verification (client-side, normative)

Given:
- Expected remote agent id: `REMOTE_ID` (from discovery / peer table)
- Peer certificate with Ed25519 public key `CERT_PUBKEY` (32 bytes)

Client verification MUST enforce:

1. `DERIVED_ID = "ed25519." + hex(SHA256(CERT_PUBKEY)[0..16])`  
   **MUST equal** `REMOTE_ID` (from SNI / intended peer id).
2. The peer must be "known" (discovered or statically configured).  
   The daemon's peer table holds a map of `agent_id → base64(pubkey)` and **rejects** if there is no entry.
3. If the peer table has an entry for `REMOTE_ID`, its base64 value **MUST** match `base64(CERT_PUBKEY)` exactly.

If any check fails: the connection is rejected at TLS verification.

#### 3.2.3 Client certificate verification (server-side, normative)

Server verification MUST enforce:

1. Extract Ed25519 public key `CERT_PUBKEY` from the client certificate.
2. Compute `DERIVED_ID` from `CERT_PUBKEY` as in §2.2.
3. Require that the daemon's peer table has an entry for `DERIVED_ID` and its value equals `base64(CERT_PUBKEY)` exactly.

If unknown (no expected key recorded), the connection is rejected.

**Operational implication:** A peer must be present in the daemon's peer table (from mDNS, static config, or cache) *before* an inbound connection will be accepted.

### 3.3 Keepalives and idle timeout

Transport parameters (RECOMMENDED values; implementations SHOULD tune for their environment):

- QUIC keepalive interval: **15 seconds** (RECOMMENDED)
- Max idle timeout: **60 seconds** (RECOMMENDED)

If idle timeout triggers, QUIC will close the connection. Implementations SHOULD reconnect following the reconnection/backoff rules (§7.2).

### 3.4 Connection limits

Max concurrent QUIC connections: **128** (hardcoded). If exceeded, new inbound QUIC connections are closed immediately with QUIC close reason "connection limit reached".

---

## 4. Stream lifecycle and mapping

AXON uses QUIC streams as **message containers**. Each stream carries exactly **one** framed application message. This is central to interoperability.

### 4.1 One-message-per-stream rule (normative)

For every AXON message:

- Sender **MUST** open a fresh QUIC stream.
- Sender **MUST** send exactly one AXON frame on that stream (§5).
- Sender **MUST** then finish its send side (`FIN`).

Receivers **MUST** read exactly one frame and then MAY ignore additional bytes (but compliant senders will not send any).

### 4.2 Stream type usage (normative)

- **Bidirectional stream (bidi)**: used for request→response.
  - Sender opens bidi stream.
  - Sender writes one framed request and finishes send side.
  - Receiver reads one request, writes one framed response on same stream, finishes send side.
- **Unidirectional stream (uni)**: used for fire-and-forget.
  - Sender opens uni stream.
  - Sender writes one framed message and finishes send side.
  - Receiver reads one message; there is no response.

### 4.3 Kind-to-stream mapping

| Kind | Stream Type | Expects Response? |
|------|-------------|-------------------|
| `request` | Bidirectional | ← `response` or `error` |
| `response` | Bidirectional (reply side) | N/A (is a response) |
| `message` | Unidirectional | No |
| `error` | Bidirectional (reply side) or Unidirectional (unsolicited) | No |

Senders MUST follow this mapping. Receivers SHOULD tolerate minor deviations gracefully.

---

## 5. Message framing (network protocol)

### 5.1 Stream-delimited framing (normative)

Each AXON message on a QUIC stream is encoded as:

- **JSON BYTES**: valid UTF-8 JSON text, written to the stream.
- **Stream FIN**: sender finishes the send side of the stream to delimit the message.

No length prefix is used. QUIC stream FIN serves as the message delimiter.

### 5.2 Max sizes and limits (normative)

- Implementations **MUST** accept messages up to at least **65,536 bytes** (64 KiB). Larger messages **MAY** be supported.
- If sender would exceed the receiver's known limit → sender **MUST** fail the send (local error).
- If receiver reads more bytes than its configured maximum → receiver **MUST** drop/abort processing for that stream.

### 5.3 Read behavior (normative)

Implementations MUST:
- Read from the stream until FIN (end of stream).
- If the stream is reset before FIN, the frame is incomplete and **MUST** be rejected/dropped.

---

## 6. JSON encoding rules (network protocol)

### 6.1 Encoding (normative)

- Character encoding: **UTF-8**, no BOM.
- Serialization: standard JSON (RFC 8259).
- Senders SHOULD emit **compact JSON** (no pretty-print). Receivers **MUST NOT** require compact formatting.
- Whitespace is permitted per JSON rules; the message size counts raw bytes including any whitespace.

### 6.2 Envelope schema (normative)

Every network message body is a JSON object:

```json
{
  "id": "uuid-v4-string",
  "kind": "request|response|message|error",
  "ref": "uuid-v4-string-or-omitted",
  "payload": { }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | UUID v4 message identifier. |
| `kind` | string | Yes | Message kind (see §4.3). |
| `ref` | string | Conditional | Referenced message ID. Present for responses. |
| `payload` | object | Yes | Kind-specific data. Unknown fields MUST be ignored. |

**Note:** `from` and `to` fields are **not** present on the wire. The daemon populates these for IPC clients based on the QUIC connection's authenticated identity.

### 6.3 `ref` field handling (interoperability note)

- When there is no reference, senders SHOULD **omit the field entirely** (not serialize `"ref": null`).
- Receivers **MUST** accept all of:
  - `"ref"` absent from the JSON object
  - `"ref": null`
  - `"ref": "<uuid>"`

### 6.4 UUID string format

- Senders SHOULD use the canonical RFC 4122 text format: `8-4-4-4-12` hex with hyphens (lowercase).
- Receivers MUST accept the canonical hyphenated form at minimum.

---

## 7. Peer pinning, reconnection

### 7.1 Peer pinning requirements (normative)

TLS verification consults the daemon's peer table, which maintains an "expected peer pubkeys" map:

- A peer's base64 Ed25519 public key **MUST** be known (via discovery, static config, or cached peers) before initiating or accepting a connection.
- If a peer is unknown at connect time, the connection is rejected at TLS verification.

### 7.2 Reconnection and backoff

Implementations SHOULD attempt reconnects to discovered peers when:
- The peer is not currently connected.

RECOMMENDED backoff schedule:
- Start: **1s**
- Then: 2s, 4s, 8s, …
- Cap: **30s** max

Implementations SHOULD use exponential backoff but specific values are implementation-defined.

---

## 8. Auto-responses (reference daemon behavior)

The reference daemon responds to unhandled bidirectional requests with `kind: "error"` and payload:

```json
{
  "code": "unhandled",
  "message": "no handler registered"
}
```

Alternative implementations need not replicate this behavior but **MUST** preserve the wire framing and message schemas.

---

## 9. Error handling on the wire

### 9.1 Malformed frames / JSON parse failures

If a receiver encounters:
- Stream reset before FIN
- Message exceeds configured max size
- JSON decode failure
- Envelope schema validation error

Then the receiver **MUST** drop the message. The connection **SHOULD NOT** be closed for a single malformed message.

### 9.2 Error envelope schema

Errors are carried as normal envelopes with `kind: "error"`:

```json
{
  "code": "<error_code>",
  "message": "Human-readable explanation",
  "retryable": false
}
```

Valid error codes:
- `unhandled`
- `peer_not_found`
- `invalid_envelope`
- `internal`
- `timeout`
- `overloaded`

`retryable` MUST be a boolean.

---

## 10. IPC wire protocol (Unix domain socket)

### 10.1 Socket location and permissions

Default socket path: `~/.axon/axon.sock`

- On startup, if the socket file exists, it is removed and re-created.
- Permissions: **0600** (owner read/write).

### 10.2 Framing: newline-delimited JSON (normative)

- Each command or reply is one JSON object terminated by `\n` (LF, `0x0A`).
- No length prefix.
- Embedded newlines inside JSON strings must be JSON-escaped (`\n`), not literal newline bytes.

### 10.3 Client → daemon commands

#### Send
```json
{"cmd":"send","to":"<agent_id>","kind":"request|message","payload":{...},"ref":"<uuid>"}
```
- `ref` is optional.
- `kind` is restricted to `request` or `message`. Other values MUST return `invalid_command`.
- For `request`, the daemon waits for the remote response and returns it inline in the `SendOk` reply.

#### Peers
```json
{"cmd":"peers"}
```

#### Status
```json
{"cmd":"status"}
```

#### Whoami
```json
{"cmd":"whoami"}
```

### 10.4 Daemon → client replies

#### SendOk

For fire-and-forget (`message`):
```json
{"ok":true,"msg_id":"<uuid>"}
```

For bidirectional requests (`request`), the response is included inline:
```json
{"ok":true,"msg_id":"<uuid>","response":{...}}
```

#### Peers
```json
{"ok":true,"peers":[{"id":"<agent_id>","addr":"ip:port","status":"connected","rtt_ms":0.4,"source":"static"}]}
```

#### Status
```json
{"ok":true,"uptime_secs":3600,"peers_connected":1,"messages_sent":42,"messages_received":38}
```

#### Whoami
```json
{"ok":true,"agent_id":"<agent_id>"}
```

#### Error
```json
{"ok":false,"error":"<error_code>","message":"<explanation>"}
```

IPC error codes:
- `invalid_command`
- `peer_not_found`
- `peer_unreachable`
- `internal_error`

#### InboundEvent (broadcast to all connected clients)
```json
{"event":"inbound","from":"<agent_id>","envelope":{...}}
```

### 10.5 Multiple IPC clients

- Multiple clients may connect simultaneously (default limit: **64**).
- If the client limit is reached, new connections are rejected.
- All connected IPC clients receive all inbound messages via broadcast as `InboundEvent` lines.
- There is no subscription mechanism and no buffering; messages are delivered as they arrive.

---

## 11. mDNS discovery (DNS-SD)

### 11.1 Service type

- Service type: **`_axon._udp.local.`**
- Underlying transport: UDP (advertises QUIC UDP port)

### 11.2 TXT record format (normative)

| Key | Value | Format |
|-----|-------|--------|
| `agent_id` | Agent ID | Typed agent ID (e.g. `ed25519.<32 hex chars>`) |
| `pubkey` | Ed25519 public key | Standard base64 (RFC 4648), no whitespace |

### 11.3 Staleness and refresh

- A discovered peer SHOULD be considered stale if no mDNS refresh within a reasonable period. RECOMMENDED: **60 seconds**.
- Stale cleanup SHOULD run periodically. RECOMMENDED: every **5 seconds**.

### 11.4 Interaction with TLS pinning

Discovery **MUST** populate the daemon's peer table used by TLS verification (§3.2). A peer's `agent_id → pubkey(base64)` mapping must be available before connections can be established.

---

## 12. Canonical constants (v1)

| Name | Value | Requirement | Description |
|------|-------|-------------|-------------|
| `MAX_MESSAGE_SIZE` | 65,536 bytes | MUST accept at least | Max JSON body size |
| Request timeout | 30 seconds | RECOMMENDED | Max wait for bidi response |
| QUIC keepalive interval | 15 seconds | RECOMMENDED | Keepalive ping interval |
| QUIC idle timeout | 60 seconds | RECOMMENDED | Max idle before connection close |
| Discovery stale timeout | 60 seconds | RECOMMENDED | Remove unrefreshed peers |
| IPC max clients | 64 | RECOMMENDED | Max concurrent socket clients |
| QUIC max connections | 128 | Hardcoded | Max concurrent QUIC connections |
| Reconnect backoff initial | 1 second | RECOMMENDED | First reconnect delay |
| Reconnect backoff max | 30 seconds | RECOMMENDED | Cap on exponential backoff |

---

## 13. Interoperability checklist

A compatible implementation MUST:

1. Use QUIC with TLS 1.3 and mutual TLS (mTLS).
2. Use Ed25519 keys in X.509 certs and derive Agent ID exactly per §2.2.
3. Verify peers by extracting Ed25519 pubkey from the presented cert and enforcing pinning (§3.2).
4. Set SNI to remote Agent ID for outbound connections (§3.2.1).
5. Use one-message-per-stream with QUIC stream FIN as delimiter — no length prefix (§4, §5).
6. Encode envelopes as UTF-8 JSON, max 65,536 bytes body (§6).
7. Implement the IPC newline-delimited JSON protocol if providing a daemon-compatible local API (§10).
8. Implement mDNS discovery `_axon._udp.local.` with required TXT keys if providing zero-config LAN discovery (§11).

---

## 14. Known interoperability notes

- `"ref"` is usually **omitted** from JSON, not `null`. Accept both.
- The reference daemon auto-responds to unhandled bidi requests with `kind: "error"`, code `"unhandled"`.
- TLS pinning requires the peer's pubkey to be known **before** connection; unknown inbound connections are rejected during TLS verification.
- Authentication is via mTLS only. Once the QUIC handshake completes successfully, the connection is fully authenticated and all message kinds are accepted immediately.
