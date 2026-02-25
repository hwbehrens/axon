# AXON Message Types

Status: Normative

_Feb 16, 2026. Simplified architecture: 4 message kinds, opaque payloads, no handshake._

## Message Kinds

AXON defines four message kinds plus a forward-compatibility sentinel:

| Kind | Stream Type | Expects Response? | Purpose |
|------|-------------|-------------------|---------|
| `request` | Bidirectional | ← `response` or `error` | Ask a peer to do something; caller blocks waiting |
| `response` | Bidirectional (reply) | — | Reply to a `request` |
| `message` | Unidirectional | No | Fire-and-forget notification |
| `error` | Bidirectional (reply) or Unidirectional (unsolicited) | No | Failure reply to a `request`, or unsolicited error |

### Forward Compatibility: `unknown`

The `MessageKind` enum uses `#[serde(other)]` to deserialize any unrecognized kind string as `Unknown`. This allows older implementations to receive messages with kinds defined in future protocol versions without failing to parse. Unknown-kind messages received on a **bidirectional** stream receive a default error response (see §Default Error Response). Unknown-kind messages received on a **unidirectional** stream are forwarded to IPC clients, allowing applications to decide how to handle future message kinds.

---

## Envelope (Wire Format)

Every QUIC message is a JSON object with exactly these fields:

```json
{
  "id": "uuid-v4",
  "kind": "request",
  "payload": { ... },
  "ref": "uuid-v4"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | UUID v4 message identifier. Must be non-nil. |
| `kind` | string | Yes | One of `request`, `response`, `message`, `error`. |
| `payload` | object | Yes | Opaque JSON object. Application-defined. |
| `ref` | string | Conditional | Referenced message ID. Present on `response` and `error` replies. Links to the original `request`'s `id`. |

**Not on the wire:** `from`, `to`, `v`, `ts`. The daemon populates `from` and `to` from the authenticated QUIC connection identity before forwarding to IPC clients.

### `ref` field handling

- When there is no reference, senders **SHOULD** omit the field entirely (not serialize `"ref": null`).
- Receivers **MUST** accept all of:
  - `"ref"` absent from the JSON object
  - `"ref": null`
  - `"ref": "<uuid>"`

---

## Stream Mapping

| Pattern | Stream | Rationale |
|---------|--------|-----------|
| `request` ↔ `response` or `error` | Bidirectional | Caller blocks waiting for reply |
| `message` | Unidirectional | Fire and forget |
| `error` (unsolicited) | Unidirectional | Connection-level or protocol-level issues |

### Rules

- **One message per stream.** Each AXON message is sent on a fresh QUIC stream. The sender writes the JSON body then finishes the send side (FIN).
- **Bidirectional streams** carry a single `request` from the initiator side, followed by a single `response` or `error` from the receiver side.
- **Unidirectional streams** carry a single `message` or unsolicited `error`. No reply is possible.
- **No hello gating.** There is no handshake exchange. Messages may be sent as soon as the QUIC/TLS connection is established and the peer's identity is verified via mTLS.

---

## Default Error Response

When a `request` arrives on a bidirectional stream and no application handler is registered (or the handler declines to respond), the daemon returns a default error:

```json
{
  "id": "uuid-v4",
  "kind": "error",
  "ref": "<original request id>",
  "payload": {
    "code": "unhandled",
    "message": "no application handler registered for request '<request-id>'",
    "retryable": false
  }
}
```

This ensures that every bidirectional request receives a reply, even if no application logic is wired up.

---

## Payloads

**Payloads are opaque JSON objects.** The AXON protocol does not define payload schemas — applications define their own conventions. The protocol treats `payload` as an arbitrary JSON object and passes it through without inspection.

This means:

- The protocol layer never validates payload contents beyond ensuring it is valid JSON.
- Applications are free to define whatever payload structure they need.
- Different agents can use different payload conventions as long as they agree amongst themselves.
- Unknown fields in payloads **MUST** be ignored (forward compatibility).

### Error Payloads

Error payloads **SHOULD** follow this conventional shape:

```json
{
  "code": "<machine-readable-code>",
  "message": "Human-readable explanation",
  "retryable": false
}
```

The `code` field is a snake\_case string. Applications may define their own error codes. The protocol itself uses:

| Code | Meaning |
|------|---------|
| `unhandled` | No handler registered for the request |

Error messages **SHOULD** be instructive — not just "failed" but an explanation of what went wrong and what the caller might try instead.

---

## Domain Conventions (Non-Normative)

Since payloads are opaque, applications may adopt domain conventions to organize interactions. A common pattern is to include a `domain` field in request payloads:

```json
{
  "kind": "request",
  "payload": {
    "domain": "family.calendar",
    "question": "What events are on the calendar this week?"
  }
}
```

Domains are dot-separated strings, conventional (not enforced by the protocol). Suggested starting taxonomy:

```
family.*          — household, family life
  .calendar       — schedules, events
  .school         — education
  .health         — medical, appointments
work.*            — professional
  .calendar       — work schedule
  .projects       — specific project domains
logistics.*       — travel, errands, shopping
  .travel         — trips, flights, hotels
  .grocery        — shopping lists
  .errands        — tasks, appointments
meta.*            — about the agents themselves
  .memory         — knowledge, recall
  .config         — agent configuration
  .status         — operational state
  .learning       — shared insights, lessons learned
```

`meta.*` is particularly powerful — it enables agents to share lessons, coordinate memory, and improve each other. No formal structure imposed; let conventions emerge from use.

---

## Learnability Design (Non-Normative)

AXON must be usable by any LLM agent with NO pre-existing training on the protocol. Design for learnability:

1. **Self-describing CLI.** `axon --help` and `axon <command> --help` must be clear enough that an LLM reading the output can use the tool correctly. Use full English words, not abbreviations.

2. **Connection bootstrap is automatic.** When two daemons discover each other (via mDNS or static config), they connect over QUIC with mutual TLS. No handshake or version negotiation is needed — the connection is ready for application messages immediately.

3. **Only four kinds.** `request`, `response`, `message`, `error`. An agent can learn the entire protocol in seconds. Requests get responses. Messages are fire-and-forget. Errors report failures.

4. **Instructive errors.** Error messages should explain what went wrong AND suggest what to do instead. Not just "failed" but "no handler registered for this request — the peer may not support this domain."

5. **`axon examples` command.** Prints annotated example interactions (`request` → `response`, fire-and-forget `message`). LLMs learn from examples faster than from specifications.

6. **Semantic field names.** `payload` not `p`. `kind` not `k`. LLMs infer meaning from names.

7. **Consistent patterns.** Every request that expects a response uses the same pattern: send `request` on a bidi stream, read `response` or `error`. Every fire-and-forget uses `message` on a uni stream. No exceptions, no special cases.
