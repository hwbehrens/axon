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
- The type prefix enables future algorithm agility. Implementations encountering an unknown type prefix **MUST** reject the peer with error code `incompatible_version`.

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

### 3.2 Initiator rule (deterministic dialing)

To prevent duplicate connections:

- The peer with the **lexicographically lower Agent ID string** (ASCII compare) is the **initiator** and **SHOULD** attempt to connect.
- If an implementation with the **higher Agent ID** needs to send but has no connection, it **SHOULD** wait briefly for the lower-ID peer to connect.

Reference daemon behavior: If higher-ID peer attempts to `send` via IPC and no connection exists, it waits **2 seconds** for an inbound connection; if still absent, it errors.

### 3.3 TLS usage and peer authentication (pinning)

AXON uses TLS 1.3 over QUIC with **mutual authentication**.

#### 3.3.1 SNI / "server name" usage (normative)

When initiating an outbound QUIC connection to a peer with Agent ID `REMOTE_ID`:

- The client **MUST** set the TLS Server Name (SNI) to the full typed Agent ID string (e.g. `ed25519.a1b2...`). The dot separator ensures the Agent ID is a valid DNS name for SNI purposes.

This SNI value is used as an *identity label* for certificate verification (not DNS).

#### 3.3.2 Server certificate verification (client-side, normative)

Given:
- Expected remote agent id: `REMOTE_ID` (from discovery / peer table)
- Peer certificate with Ed25519 public key `CERT_PUBKEY` (32 bytes)

Client verification MUST enforce:

1. `DERIVED_ID = "ed25519." + hex(SHA256(CERT_PUBKEY)[0..16])`  
   **MUST equal** `REMOTE_ID` (from SNI / intended peer id).
2. The peer must be "known" (discovered or statically configured).  
   The client holds a map: `expected_pubkeys[agent_id] = base64(pubkey)` and **rejects** if there is no entry.
3. If `expected_pubkeys[REMOTE_ID]` exists, its base64 value **MUST** match `base64(CERT_PUBKEY)` exactly.

If any check fails: the connection is rejected at TLS verification.

#### 3.3.3 Client certificate verification (server-side, normative)

Server verification MUST enforce:

1. Extract Ed25519 public key `CERT_PUBKEY` from the client certificate.
2. Compute `DERIVED_ID` from `CERT_PUBKEY` as in §2.2.
3. Require that `expected_pubkeys[DERIVED_ID]` exists and equals `base64(CERT_PUBKEY)` exactly.

If unknown (no expected key recorded), the connection is rejected.

**Operational implication:** A peer must be present in the expected peer table (from mDNS, static config, or cache) *before* an inbound connection will be accepted.

### 3.4 Keepalives and idle timeout

Transport parameters (RECOMMENDED values; implementations SHOULD tune for their environment):

- QUIC keepalive interval: **15 seconds** (RECOMMENDED)
- Max idle timeout: **60 seconds** (RECOMMENDED)

If idle timeout triggers, QUIC will close the connection. Implementations SHOULD reconnect following the reconnection/backoff rules (§7.3).

### 3.5 Connection limits

Max concurrent QUIC connections (default): **128** (configurable). If exceeded, new inbound QUIC connections are closed immediately with QUIC close reason "connection limit reached".

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
| `hello` | Bidirectional | ← `hello` |
| `ping` | Bidirectional | ← `pong` |
| `query` | Bidirectional | ← `response` or `error` |
| `delegate` | Bidirectional | ← `ack` or `error` |
| `cancel` | Bidirectional | ← `ack` or `error` |
| `discover` | Bidirectional | ← `capabilities` or `error` |
| `notify` | Unidirectional | No |
| `result` | Unidirectional | No |
| `error` (unsolicited) | Unidirectional | No |

Senders SHOULD follow this mapping. Receivers SHOULD tolerate minor deviations gracefully (e.g., a fire-and-forget kind on a bidi stream will receive an `error(unknown_kind)` response).

### 4.4 Hello gating (normative)

On any newly established QUIC connection, a peer is considered **unauthenticated** at the AXON application layer until a valid **`hello` exchange** completes.

1. **Unidirectional streams received before successful hello**: The receiver **MUST drop** (ignore) these messages and **MUST NOT** forward them to IPC.
2. **Bidirectional request received before successful hello** (and not itself a `hello`): The receiver **MUST** respond with an `error` envelope with code `not_authorized` and message `"hello handshake must complete before other requests"`.

Once hello completes, all subsequent messages from the authenticated peer are processed normally.

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
  "v": 1,
  "id": "uuid-v4-string",
  "from": "ed25519.32-hex-char-agent-id",
  "to": "ed25519.32-hex-char-agent-id",
  "ts": 1771108000000,
  "kind": "hello",
  "ref": "uuid-v4-string-or-omitted",
  "payload": { }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `v` | u8 | Yes | Protocol version. MUST be non-zero. Currently `1`. |
| `id` | string | Yes | UUID v4 message identifier. |
| `from` | string | Yes | Sender typed agent ID (e.g. `ed25519.<32 hex chars>`). |
| `to` | string | Yes | Receiver typed agent ID (e.g. `ed25519.<32 hex chars>`). |
| `ts` | u64 | Yes | Unix timestamp in milliseconds. MUST be non-zero. |
| `kind` | string | Yes | Message kind (see §4.3). |
| `ref` | string | Conditional | Referenced message ID. Present for responses. |
| `payload` | object | Yes | Kind-specific data. Unknown fields MUST be ignored. |

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

## 7. Message ordering, constraints, replay protection, reconnection

### 7.1 Hello must be first (normative)

On a newly established QUIC connection:

- Initiator **MUST** send `hello` as its first request.
- Receiver **MUST** reject or ignore all non-hello traffic until it has accepted a valid hello.

See §4.4 for detailed enforcement behavior.

### 7.2 Peer pinning requirements (normative)

TLS verification consults an "expected peer pubkeys" map:

- A peer's base64 Ed25519 public key **MUST** be known (via discovery, static config, or cached peers) before initiating or accepting a connection.
- If a peer is unknown at connect time, the connection is rejected at TLS verification.

### 7.3 Reconnection and backoff

Implementations SHOULD attempt reconnects to peers when:
- `local_agent_id < peer.agent_id` (initiator rule), AND
- peer is not currently connected.

RECOMMENDED backoff schedule:
- Start: **1s**
- Then: 2s, 4s, 8s, …
- Cap: **30s** max

Implementations SHOULD use exponential backoff but specific values are implementation-defined.

### 7.4 Replay protection (UUID de-dup, normative)

AXON provides application-layer replay protection using envelope IDs:

- Cache key: `envelope.id` (UUID string).
- If an inbound envelope ID has been seen within TTL, it is a **replay** and **MUST** be dropped (not forwarded to IPC).

TTL: **300 seconds** (5 minutes).

Persistence format (`~/.axon/replay_cache.json`):
```json
[{"id":"<uuid>","seen_at_ms":1234567890}, ...]
```

On startup, entries older than TTL are discarded.

**Note:** Replay protection is best-effort, intended primarily to mitigate QUIC 0-RTT replay and duplicates after reconnects. It is not a cryptographic nonce scheme.

---

## 8. Hello handshake details (application layer)

### 8.1 Hello request

The initiator sends a `hello` envelope on a fresh bidirectional stream.

- `kind` MUST be `"hello"`.
- `from` MUST be the sender's agent ID.
- `to` MUST be the intended peer's agent ID.
- `payload.protocol_versions` MUST include `1` to interoperate with v1.
- `payload.selected_version` MUST be absent in the request.

Payload schema:
```json
{
  "protocol_versions": [1],
  "agent_name": "optional display name",
  "features": ["delegate", "ack", "result", "cancel", "discover", "capabilities"]
}
```

### 8.2 Hello identity validation (receiver-side, normative)

Upon receiving a `hello` request, receiver MUST validate:

1. Extract peer certificate's Ed25519 public key from the QUIC connection's TLS identity.
2. Derive `DERIVED_ID` from cert pubkey per §2.2.
3. Require: `DERIVED_ID == hello.from`.
4. If receiver has a pinned expected pubkey for `hello.from`, require it matches the cert pubkey (base64).

If validation fails, receiver responds with `error` and MUST NOT mark the connection authenticated.

### 8.3 Hello response (success)

If `protocol_versions` overlap includes v1:
- Receiver responds with `kind: "hello"` on the same bidi stream.
- `ref` MUST be set to the request's `id`.
- `payload.selected_version` = `1`.
- `payload.protocol_versions` = `[1]`.
- `payload.features` = list of supported optional kinds.

If no version overlap:
- Receiver MUST respond with `error`, code `incompatible_version`.

### 8.4 Auto-responses (reference daemon behavior)

The reference daemon auto-responds to bidi requests after authentication:

| Request Kind | Response Kind | Behavior |
|-------------|--------------|----------|
| `ping` | `pong` | `{"status":"idle","uptime_secs":0,"active_tasks":0}` |
| `discover` | `capabilities` | Placeholder capability data |
| `query` | `response` | Indicates no handler registered |
| `delegate` | `ack` | `{"accepted":true}` |
| `cancel` | `ack` | `{"accepted":true}` |
| Unknown kind | `error` | `code: "unknown_kind"` |

Alternative implementations need not replicate these placeholder semantics but **MUST** preserve the wire framing, hello gating, and message schemas.

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
- `not_authorized`
- `unknown_domain`
- `overloaded`
- `internal`
- `timeout`
- `cancelled`
- `incompatible_version`
- `unknown_kind`
- `peer_not_found`
- `invalid_envelope`

`retryable` MUST be a boolean.

---

## 10. IPC wire protocol (Unix domain socket)

> **Normative reference:** IPC v2 is fully specified in [`spec/IPC.md`](IPC.md).
> This section covers the baseline wire shapes common to both v1 and v2.
> Where this section and `spec/IPC.md` conflict, `spec/IPC.md` takes precedence.

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
{"cmd":"send","to":"<agent_id>","kind":"<message_kind>","payload":{...},"ref":"<uuid>"}
```
- `ref` is optional.

#### Peers
```json
{"cmd":"peers"}
```

#### Status
```json
{"cmd":"status"}
```

> **v2 additions:** IPC v2 adds `hello`, `auth`, `whoami`, `inbox`, `ack`, and `subscribe` commands.
> For v2 connections, `req_id` is mandatory on all commands except `hello`.
> See [`spec/IPC.md`](IPC.md) §3 for full schemas.

### 10.4 Daemon → client replies

#### SendAck
```json
{"ok":true,"msg_id":"<uuid>"}
```

#### Peers
```json
{"ok":true,"peers":[{"id":"<agent_id>","addr":"ip:port","status":"connected","rtt_ms":0.4,"source":"static"}]}
```

#### Status
```json
{"ok":true,"uptime_secs":3600,"peers_connected":1,"messages_sent":42,"messages_received":38}
```

#### Error

v1:
```json
{"ok":false,"error":"peer_not_found"}
```

v2 (includes `req_id`):
```json
{"ok":false,"req_id":"r1","error":"peer_not_found"}
```

The `error` field MUST be a machine-readable `IpcErrorCode` (snake\_case).
See [`spec/IPC.md`](IPC.md) §5 for the full list of error codes.

#### Inbound message (broadcast to all connected clients)
```json
{"inbound":true,"envelope":{...}}
```

### 10.5 Multiple IPC clients

- Multiple clients may connect simultaneously (default limit: **64**).
- If the client limit is reached, new connections are rejected.

**Delivery rules by connection type:**

| Connection type | Inbound delivery |
|---|---|
| **v1** (no `hello`) | Legacy broadcast: all inbound messages pushed to all connected v1 clients. |
| **v2, no subscription** | No unsolicited inbound messages. Client MUST use `inbox` (pull) to retrieve buffered messages. |
| **v2, active subscription** | Inbound messages matching the subscription filter are pushed as `{"event": "inbound", ...}` lines. |

v1 broadcast and v2 subscriptions are separate delivery paths. See [`spec/IPC.md`](IPC.md) §3.5.

**Note:** Subscribe replay bursts are bounded by the receive buffer capacity (`ipc.buffer_size`, default 1000 messages). See [`spec/IPC.md`](IPC.md) §3.5 for details.

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

Discovery **MUST** populate the expected peer table used by TLS verification (§3.3). A peer's `agent_id → pubkey(base64)` mapping must be available before connections can be established.

---

## 12. Canonical constants (v1)

| Name | Value | Requirement | Description |
|------|-------|-------------|-------------|
| `MAX_MESSAGE_SIZE` | 65,536 bytes | MUST accept at least | Max JSON body size |
| Request timeout | 30 seconds | RECOMMENDED | Max wait for bidi response |
| QUIC keepalive interval | 15 seconds | RECOMMENDED | Keepalive ping interval |
| QUIC idle timeout | 60 seconds | RECOMMENDED | Max idle before connection close |
| Replay cache TTL | 300 seconds | RECOMMENDED | De-dup window for message IDs |
| Discovery stale timeout | 60 seconds | RECOMMENDED | Remove unrefreshed peers |
| IPC max clients | 64 | RECOMMENDED | Max concurrent socket clients |
| QUIC max connections | 128 | RECOMMENDED | Max concurrent QUIC connections |
| Initiator wait timeout | 2 seconds | RECOMMENDED | Higher-ID waits for inbound connection |
| Reconnect backoff initial | 1 second | RECOMMENDED | First reconnect delay |
| Reconnect backoff max | 30 seconds | RECOMMENDED | Cap on exponential backoff |

---

## 13. Interoperability checklist

A compatible implementation MUST:

1. Use QUIC with TLS 1.3 and mutual TLS (mTLS).
2. Use Ed25519 keys in X.509 certs and derive Agent ID exactly per §2.2.
3. Verify peers by extracting Ed25519 pubkey from the presented cert and enforcing pinning (§3.3).
4. Set SNI to remote Agent ID for outbound connections (§3.3.1).
5. Use one-message-per-stream with QUIC stream FIN as delimiter — no length prefix (§4, §5).
6. Encode envelopes as UTF-8 JSON, max 65,536 bytes body (§6).
7. Enforce hello gating: drop unauthenticated uni; error on unauthenticated bidi non-hello (§4.4).
8. Implement replay dedup by envelope UUID with 300s TTL (§7.4).
9. Implement the IPC newline-delimited JSON protocol if providing a daemon-compatible local API (§10).
10. Implement mDNS discovery `_axon._udp.local.` with required TXT keys if providing zero-config LAN discovery (§11).

---

## 14. Known interoperability notes

- `"ref"` is usually **omitted** from JSON, not `null`. Accept both.
- Unauthenticated **uni** messages are silently dropped (no error response possible).
- The reference daemon auto-responds to bidi requests even when IPC clients are connected.
- TLS pinning requires the peer's pubkey to be known **before** connection; unknown inbound connections are rejected during TLS verification (before any hello exchange).
