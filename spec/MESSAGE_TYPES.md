# AXON Message Types — v2 (Revised)

_Feb 14, 2026. Incorporates feedback. Personal details scrubbed for eventual public release._

## Message Kinds

### Core Types

| Kind | Expects Response? | Stream Type | Purpose |
|------|-------------------|-------------|---------|
| `hello` | ← `hello` | Bidirectional | Version negotiation + identity exchange (first message on any new connection) |
| `ping` | ← `pong` | Bidirectional | Liveness check |
| `query` | ← `response` | Bidirectional | Ask a question |
| `delegate` | ← `ack`, then optionally ← `result` | Bidir (ack), then Unidir (result) | Assign a task |
| `notify` | No | Unidirectional | Informational, fire-and-forget |
| `cancel` | ← `ack` | Bidirectional | Cancel a previous delegation |
| `discover` | ← `capabilities` | Bidirectional | What can you do? |

### Response Types

| Kind | Purpose |
|------|---------|
| `hello` | Version negotiation response |
| `pong` | Heartbeat response with status |
| `response` | Answer to a query |
| `ack` | Acknowledgment (delegation/cancel received) |
| `result` | Async task completion report |
| `capabilities` | Response to discover |
| `error` | Failure response to any request |

---

## Version Negotiation: `hello`

**Every new QUIC connection begins with a `hello` exchange.** No other messages may be sent until `hello` completes.

The initiating peer opens a bidirectional stream and sends:
```json
{
  "v": 1,
  "id": "uuid",
  "from": "agent-id",
  "to": "agent-id",
  "ts": 1771108000000,
  "kind": "hello",
  "ref": null,
  "payload": {
    "protocol_versions": [1],
    "agent_name": "optional display name",
    "features": ["delegate", "discover"]
  }
}
```

The receiver responds on the same stream:
```json
{
  "v": 1,
  "id": "uuid",
  "from": "agent-id",
  "to": "agent-id",
  "ts": 1771108000001,
  "kind": "hello",
  "ref": "original-hello-id",
  "payload": {
    "protocol_versions": [1],
    "selected_version": 1,
    "agent_name": "optional display name",
    "features": ["delegate", "discover", "cancel"]
  }
}
```

- `protocol_versions`: array of supported protocol versions (ascending).
- `selected_version`: highest mutually supported version. If no overlap → `error` with code `incompatible_version`.
- `features`: optional list of supported message kinds beyond the required set (`ping`, `query`, `notify`, `error`). Allows graceful degradation — if a peer doesn't advertise `delegate`, don't send delegations.

**Required kinds** (must be supported by all versions): `hello`, `ping`, `pong`, `query`, `response`, `notify`, `error`.

**Optional kinds** (advertised in features): `delegate`, `ack`, `result`, `cancel`, `discover`, `capabilities`.

---

## Envelope (all messages)

```json
{
  "v": 1,
  "id": "uuid-v4",
  "from": "agent-id-hex",
  "to": "agent-id-hex",
  "ts": 1771108000000,
  "kind": "<message kind>",
  "ref": "<referenced message id, or null>",
  "payload": { ... }
}
```

- `v`: protocol version (negotiated via hello).
- `id`: unique message identifier (UUID v4).
- `from` / `to`: typed agent IDs (e.g. `ed25519.` + first 16 bytes of SHA-256 of public key, hex, 40 chars total).
- `ts`: unix milliseconds.
- `kind`: message type string.
- `ref`: the message ID this responds to. Null for initiating messages.
- `payload`: kind-specific data. Unknown fields MUST be ignored (forward compatibility).

---

## Payload Schemas

### ping
```json
{ }
```

### pong
```json
{
  "status": "idle|busy|overloaded",
  "uptime_secs": 3600,
  "active_tasks": 2,
  "agent_name": "display name"
}
```

### query
```json
{
  "question": "What events are on the family calendar this week?",
  "domain": "family.calendar",
  "max_tokens": 200,
  "deadline_ms": 30000
}
```
- `question`: natural language or structured query.
- `domain`: dot-separated topic hint. Optional. Helps receiver scope its answer.
- `max_tokens`: numeric budget for the response. 0 = no limit. Receiver should treat as a guideline.
- `deadline_ms`: how long sender waits before timing out. Optional, default 30000.

### response
```json
{
  "data": { "events": [ ... ] },
  "summary": "Three swim practices this week: Mon/Wed/Fri 4-5pm",
  "tokens_used": 47,
  "truncated": false
}
```
- `data`: structured response (any JSON value).
- `summary`: human-readable one-liner. Always present.
- `tokens_used`: budget consumed. Informational.
- `truncated`: whether response was cut short due to max_tokens.

### delegate
```json
{
  "task": "Send a message to the family group chat about dinner plans",
  "context": {
    "dinner_time": "7:00 PM",
    "location": "home"
  },
  "priority": "normal|urgent",
  "report_back": true,
  "deadline_ms": 60000
}
```
- `task`: natural language task description.
- `context`: structured data the receiver needs. Arbitrary JSON object. Optional.
- `priority`: `normal` or `urgent`. Default `normal`.
- `report_back`: should receiver send a `result` when done? Default true. If false, delegate is fire-and-forget after ack.
- `deadline_ms`: task should be attempted within this window. Optional.

### ack
```json
{
  "accepted": true,
  "estimated_ms": 5000
}
```
- `accepted`: will the receiver attempt the task? False = refused.
- `estimated_ms`: rough completion estimate. Optional.

### result
```json
{
  "status": "completed|failed|partial",
  "outcome": "Message sent to the group chat. Got a thumbs-up reaction.",
  "data": { ... },
  "error": null
}
```
- `status`: completion state.
- `outcome`: natural language summary.
- `data`: structured result. Optional.
- `error`: error message if failed. Null on success.

Note: `result` does NOT include the original task description. The sender correlates via `ref`.

### notify
```json
{
  "topic": "user.location",
  "data": {
    "status": "heading out",
    "eta_back": "2h"
  },
  "importance": "low|medium|high"
}
```
- `topic`: dot-separated topic string.
- `data`: arbitrary payload.
- `importance`: hint. `low` = background, `high` = act on this. Default `low`.

No subscription mechanism in v0.2. Senders push; receivers filter.

### cancel
```json
{
  "reason": "Plans changed, no longer needed"
}
```
`ref` field in envelope points to the delegation being cancelled. Best-effort.

### discover
```json
{ }
```

### capabilities
```json
{
  "agent_name": "Family Assistant",
  "domains": ["family", "calendar", "groceries", "school"],
  "channels": ["imessage", "apple-reminders"],
  "tools": ["web_search", "calendar_cli"],
  "max_concurrent_tasks": 4,
  "model": "gemini-3-pro"
}
```
All fields optional. Agents share what they choose.

### error
```json
{
  "code": "not_authorized|unknown_domain|overloaded|internal|timeout|cancelled|incompatible_version|unknown_kind|peer_not_found|invalid_envelope",
  "message": "Human-readable explanation of what went wrong and what to try instead",
  "retryable": false
}
```

Error messages SHOULD be instructive. Not just "permission denied" but "I don't have access to work calendars. Try querying the work agent (agent ID a1b2...) instead."

---

## Stream Mapping

| Pattern | Stream | Rationale |
|---------|--------|-----------|
| hello ↔ hello | Bidirectional | Must complete before anything else |
| ping ↔ pong | Bidirectional | Fast round-trip |
| query ↔ response/error | Bidirectional | Caller blocks waiting |
| delegate ↔ ack/error | Bidirectional | Caller blocks for ack only |
| result | Unidirectional | Async, may arrive much later |
| notify | Unidirectional | Fire and forget |
| cancel ↔ ack/error | Bidirectional | Confirm cancellation |
| discover ↔ capabilities/error | Bidirectional | Round-trip |
| unsolicited error | Unidirectional | Connection-level issues |

---

## Domain Conventions

Domains are dot-separated, conventional (not enforced). Suggested starting taxonomy:

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

2. **Connection bootstrap is automatic.** When two daemons connect over QUIC, they exchange `hello` messages automatically — the agent does not need to initiate this. The `hello` handshake negotiates protocol version and advertises supported features. No other messages can be sent until `hello` completes.

3. **`axon discover` is the first agent action.** After the daemon has connected and completed `hello`, the agent's first action should be `axon discover <agent>` to learn what a peer can do. The `capabilities` response gives the agent a complete map of that peer's domains, tools, and capacity.

4. **Instructive errors.** Error messages should explain what went wrong AND suggest what to do instead. "Unknown domain 'work.calendar'. This agent handles: family, calendar, groceries. Try querying agent b3c4... for work domains."

5. **`axon examples` command.** Prints a complete annotated example interaction (hello → discover → query → response). LLMs learn from examples faster than from specifications.

6. **Semantic field names.** `question` not `q`. `report_back` not `rb`. `max_tokens` not `mt`. LLMs infer meaning from names.

7. **OpenClaw SKILL.md.** When installed as a skill, the description and SKILL.md teach the agent the patterns: "You can talk to other agents using AXON. Start with `axon peers` to see who's available, then `axon discover <agent>` to learn what they can do."

8. **Consistent patterns.** Every request that expects a response uses the same pattern: send on bidir stream, read response. Every fire-and-forget uses unidir. No exceptions, no special cases.
