# ACP Message Types — Deep Design Draft

_Feb 14, 2026 — Kit thinking through what agents actually need to say to each other._

## Design Constraints

1. **Schemas must be parseable by LLMs.** Agents generate and consume these. Complex nesting or ambiguous fields = errors.
2. **Schemas must be extensible.** We can't predict every future use case. Unknown fields should be ignored, not rejected.
3. **Every message that expects a response must be correlatable.** The sender needs to match responses to requests.
4. **Fail-fast semantics.** No queuing, no guaranteed delivery. If it doesn't arrive, the sender gets an error.

## Thinking About What Agents Actually Say

From my experience running inside OpenClaw, agent-to-agent communication falls into these patterns:

### Pattern 1: "What do you know about X?"
Kit → Family Agent: "What are the kids' swim schedules this week?"
Family Agent → Kit: structured schedule data

This is a **query**. Key properties:
- Has a specific question
- Expects a structured response
- Caller controls how much detail they want (token budget)
- May specify a domain to help the responder scope their answer

### Pattern 2: "Do this for me"
Kit → Family Agent: "Tell the family contact that the user will be late for dinner"
Family Agent → Kit: "Done, she said okay"

This is a **delegation**. Key properties:
- Describes a task, not a question
- May include context the other agent needs
- May or may not want a report back
- Has a priority (is this urgent?)
- The other agent may **refuse** (can't do it, not authorized, etc.)

### Pattern 3: "FYI"
Kit → Family Agent: "the user just left for the gym, back in 2 hours"
(no response expected)

This is a **notification**. Key properties:
- Informational, no response expected
- Has a topic (for the receiver to categorize/filter)
- Should be idempotent (receiving twice is fine)

### Pattern 4: "Are you there?"
Kit → Family Agent: ping
Family Agent → Kit: pong (with status)

This is a **heartbeat**. Key properties:
- Liveness check
- Response includes basic status (busy/idle, current load, uptime)
- Should be very cheap (minimal tokens, fast response)

### Pattern 5: "What can you do?"
Kit → Family Agent: "What domains/capabilities do you handle?"
Family Agent → Kit: { domains: ["family", "calendar", "groceries"], capabilities: ["imessage", "apple-reminders"] }

This is **capability discovery**. Key properties:
- Lets agents learn what others can do
- Enables intelligent routing ("who should I ask about swim schedules?")
- Should be cacheable (capabilities don't change often)

### Pattern 6: "Never mind"
Kit → Family Agent: "Cancel task abc123"
Family Agent → Kit: "Acknowledged, cancelled"

This is a **cancellation**. Key properties:
- References a previous delegation by message ID
- Best-effort (task may already be complete)

### Pattern 7: "Something went wrong"
Family Agent → Kit: "Task abc123 failed: the family contact's phone is off"

This is an **error/status update** on a previously delegated task. Key properties:
- References a previous message
- Carries error information or progress updates

## Proposed Message Types

After thinking through the patterns, here's what I'd propose:

### Core Types (v0.2)

| Kind | Direction | Response? | Purpose |
|------|-----------|-----------|---------|
| `ping` | → | ← `pong` | Liveness + basic status |
| `query` | → | ← `response` | Ask a question, get an answer |
| `delegate` | → | ← `ack` then optionally `result` | Assign a task |
| `notify` | → | (none) | Informational broadcast |
| `cancel` | → | ← `ack` | Cancel a previous delegation |
| `discover` | → | ← `capabilities` | What can you do? |

### Response/Async Types

| Kind | Direction | Purpose |
|------|-----------|---------|
| `pong` | ← | Heartbeat response |
| `response` | ← | Answer to a query |
| `ack` | ← | Acknowledgment (delegation received, cancel received) |
| `result` | ← | Task completion report (async, may arrive much later) |
| `capabilities` | ← | Response to discover |
| `error` | ← | Something went wrong (response to any request type) |

## Schemas

### Envelope (all messages)
```json
{
  "v": 1,
  "id": "uuid-v4",
  "from": "agent-id-hex",
  "to": "agent-id-hex",
  "ts": 1771108000000,
  "kind": "query|delegate|notify|ping|cancel|discover|pong|response|ack|result|capabilities|error",
  "ref": null,
  "payload": { ... }
}
```

`ref` (reference): the message ID this is responding to. Null for initiating messages. Set for all response types. This is how senders correlate responses to requests.

### ping
```json
{
  "kind": "ping"
}
```
No payload. Just knock.

### pong
```json
{
  "kind": "pong",
  "ref": "original-ping-id",
  "payload": {
    "status": "idle|busy|overloaded",
    "uptime_secs": 3600,
    "active_tasks": 2,
    "agent_name": "Kit"
  }
}
```

### query
```json
{
  "kind": "query",
  "payload": {
    "question": "What are the kids' swim schedules this week?",
    "domain": "family.calendar",
    "max_tokens": 200,
    "deadline_ms": 30000
  }
}
```

- `question`: natural language or structured query
- `domain`: dot-separated topic hint (helps receiver scope). Optional.
- `max_tokens`: numeric budget for the response. Receiver should respect this as a guideline. 0 = no limit.
- `deadline_ms`: how long the sender will wait before timing out. Optional. Default: 30000.

### response
```json
{
  "kind": "response",
  "ref": "original-query-id",
  "payload": {
    "data": { ... },
    "summary": "Three swim practices: Mon/Wed/Fri 4-5pm at the local pool",
    "tokens_used": 47,
    "truncated": false
  }
}
```

- `data`: structured response (any JSON value)
- `summary`: human-readable one-liner. Always present even if data is complex.
- `tokens_used`: how many tokens of budget were consumed. Informational.
- `truncated`: whether the response was cut short due to max_tokens.

### delegate
```json
{
  "kind": "delegate",
  "payload": {
    "task": "Send the family contact a message saying the user will be 30 minutes late for dinner",
    "context": {
      "reason": "Meeting ran over",
      "dinner_time": "7:00 PM",
      "grace_handle": "family-contact@example.com"
    },
    "priority": "normal|urgent",
    "report_back": true,
    "deadline_ms": 60000
  }
}
```

- `task`: natural language task description
- `context`: structured data the receiver needs. Arbitrary JSON object.
- `priority`: `normal` or `urgent`. Urgent = interrupt current work if needed.
- `report_back`: should the receiver send a `result` when done?
- `deadline_ms`: task should be attempted within this window. Optional.

### ack
```json
{
  "kind": "ack",
  "ref": "original-delegate-id",
  "payload": {
    "accepted": true,
    "estimated_ms": 5000
  }
}
```

- `accepted`: whether the receiver will attempt the task. False = refused (with reason in error).
- `estimated_ms`: rough estimate of completion time. Optional.

Sent immediately upon receiving a delegation. The actual result comes later as a `result` message.

### result
```json
{
  "kind": "result",
  "ref": "original-delegate-id",
  "payload": {
    "status": "completed|failed|partial",
    "outcome": "Message sent to the family contact. She replied 'ok no worries'",
    "data": { ... },
    "error": null
  }
}
```

- `status`: did it work?
- `outcome`: natural language summary of what happened
- `data`: structured result data. Optional.
- `error`: error message if failed. Null on success.

### notify
```json
{
  "kind": "notify",
  "payload": {
    "topic": "hans.location",
    "data": {
      "location": "heading to gym",
      "eta_back": "2h"
    },
    "importance": "low|medium|high"
  }
}
```

- `topic`: dot-separated topic string. Receiver can filter/categorize.
- `data`: arbitrary payload.
- `importance`: hint for the receiver. `low` = background context, `high` = act on this.

### cancel
```json
{
  "kind": "cancel",
  "ref": "original-delegate-id",
  "payload": {
    "reason": "Plans changed, no longer needed"
  }
}
```

### discover
```json
{
  "kind": "discover"
}
```
No payload. "Tell me what you can do."

### capabilities
```json
{
  "kind": "capabilities",
  "ref": "original-discover-id",
  "payload": {
    "agent_name": "Family Assistant",
    "domains": ["family", "calendar", "groceries", "school", "activities"],
    "channels": ["imessage", "apple-reminders"],
    "tools": ["web_search", "gog", "bluebubbles"],
    "max_concurrent_tasks": 4,
    "model": "gemini-3-pro"
  }
}
```

### error
```json
{
  "kind": "error",
  "ref": "original-message-id",
  "payload": {
    "code": "not_authorized|unknown_domain|overloaded|internal|timeout|cancelled",
    "message": "I don't have access to the user's work calendar",
    "retryable": false
  }
}
```

## Stream Mapping

Going back to the bidirectional vs unidirectional question:

**Proposal: Use the kind to determine stream type.**

| Pattern | Stream Type | Why |
|---------|------------|-----|
| ping → pong | Bidirectional | Simple, fast, one round trip |
| query → response | Bidirectional | Caller waits synchronously |
| delegate → ack | Bidirectional | Caller waits for ack |
| delegate ... result | Unidirectional (result sent separately) | Async, may arrive much later |
| notify | Unidirectional | Fire and forget |
| cancel → ack | Bidirectional | Caller confirms cancellation |
| discover → capabilities | Bidirectional | Simple round trip |
| error | Unidirectional | Can be sent at any time for any ref |

The key insight: **synchronous exchanges** (where the caller blocks waiting) use bidirectional streams. **Asynchronous messages** (results, notifications, errors) use unidirectional streams. This maps naturally to the semantics — you open a bidir stream when you need an answer now, and fire a unidir stream when you're pushing information.

## Domain Conventions

Domains are dot-separated strings. Some proposed conventions:

```
family.*          — family life, household
family.calendar   — schedules, events
family.school     — kids' school stuff
family.health     — medical, appointments
work.*            — professional
work.calendar     — work schedule
work.plasma       — Plasma-specific
logistics.*       — travel, errands, shopping
logistics.travel  — trips, flights
logistics.grocery — shopping lists
meta.*            — about the agents themselves
meta.memory       — memory/knowledge queries
meta.config       — configuration
meta.status       — operational status
```

Domains are hints, not enforcement. An agent can answer queries outside its declared domains.

## Versioning

- Envelope `v` field is the protocol version.
- Unknown `kind` values: respond with `error` (code: `unknown_kind`).
- Unknown payload fields: **ignore** (forward-compatible).
- Version negotiation: not in v0.2. If we ever need it, add a `handshake` message type.

## Open Design Questions

1. **Should `result` messages include the original task description?** Pro: receiver doesn't need to look up the original delegation. Con: duplicated data, wastes tokens.

2. **Should there be a `progress` message type?** For long-running delegations, the receiver could send periodic progress updates. Might be overkill for v0.2.

3. **Should `notify` support topic subscriptions?** Agent A says "I want all `hans.location` updates." Agent B remembers and pushes them. Or just push everything and let the receiver filter?

4. **Error codes**: Is the proposed set sufficient? What am I missing?

5. **Domain registry**: Should domains be formally registered or purely conventional? I lean conventional — formality adds overhead without clear benefit at our scale.
