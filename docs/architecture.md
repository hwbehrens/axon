# ACP Architecture Exploration

_Draft v0.1 — Feb 14, 2026_

## Communication Patterns

### Pattern 1: Request/Response (RPC-style)
```
Kit → Agent Two: { type: "query", domain: "calendar", question: "What are the kids' swim schedules this week?" }
Agent Two → Kit: { type: "response", data: [...structured calendar events...], summary: "Three swim practices: Mon/Wed/Fri 4-5pm" }
```
**Pros:** Simple, stateless, easy to reason about.
**Cons:** Doesn't handle ongoing collaboration well. Each request is isolated.

### Pattern 2: Shared Context Board
A shared file or lightweight DB that both agents read/write:
```
/shared/context-board.json
{
  "hans": {
    "location": "home",
    "lastSeen": "2026-02-14T13:00:00",
    "currentFocus": "family time",
    "updatedBy": "kit"
  },
  "familyCalendar": {
    "nextEvent": "Swim practice 4pm",
    "updatedBy": "agent-two",
    "updatedAt": "2026-02-14T12:00:00"
  }
}
```
**Pros:** No round-trips for shared state. Both agents stay current passively.
**Cons:** Conflict resolution. Who wins on simultaneous writes?

### Pattern 3: Event Stream
Agents publish events to a shared local stream (like a minimal Kafka):
```
[14:00] kit: { event: "hans_requested_swim_schedule", routing: "agent-two" }
[14:01] agent-two: { event: "swim_schedule_response", data: [...], routing: "kit" }
[14:05] agent-two: { event: "grace_confirmed_dinner_7pm", broadcast: true }
[14:06] kit: { event: "calendar_updated", detail: "added dinner 7pm", broadcast: true }
```
**Pros:** Async, decoupled, supports broadcast. Full audit trail.
**Cons:** More infrastructure. Ordering guarantees needed.

### Recommendation: Hybrid (Pattern 1 + 2)
- **Shared Context Board** for ambient state (who's where, what's happening, calendar snapshots)
- **RPC** for direct queries and task delegation
- Keep it simple. Event streaming is overkill for two agents.

## Message Format

### Envelope
```json
{
  "v": 1,
  "id": "msg_abc123",
  "from": "kit",
  "to": "agent-two",
  "ts": 1771098000000,
  "type": "request|response|notify|sync",
  "replyTo": null,
  "ttl": 300,
  "payload": { ... }
}
```

### Payload Types

#### query — Ask the other agent something
```json
{
  "kind": "query",
  "domain": "calendar|family|logistics|general",
  "question": "structured or natural language",
  "contextBudget": "minimal|standard|full",
  "responseFormat": "structured|summary|both"
}
```

`contextBudget` tells the responder how much detail to include:
- `minimal`: one-line answer, no context
- `standard`: answer + relevant context (~200 tokens)  
- `full`: everything you know about this topic

#### delegate — Ask the other agent to do something
```json
{
  "kind": "delegate",
  "task": "Send Grace a message about dinner plans",
  "context": { "dinnerTime": "7pm", "restaurant": "none, cooking at home" },
  "priority": "normal|urgent",
  "reportBack": true
}
```

#### notify — Broadcast information (no response expected)
```json
{
  "kind": "notify",
  "topic": "hans.location",
  "data": { "location": "heading to gym", "eta_back": "2h" },
  "relevance": "low|medium|high"
}
```

#### sync — Share/request shared state
```json
{
  "kind": "sync",
  "scope": "calendar|family|all",
  "since": 1771090000000,
  "entries": [ ... ]
}
```

## Transport Layer

### Option A: Unix Domain Socket
- Zero network overhead (same machine) or TCP on LAN
- Persistent connection, bidirectional
- Simple: just newline-delimited JSON over a socket
- **Best for same-machine deployment**

### Option B: HTTP/SSE
- Each agent exposes a lightweight HTTP endpoint
- POST for requests, SSE for event stream
- Works across machines on LAN
- Slightly more overhead but uses existing infra
- **Best for separate-machine deployment** ← our case

### Option C: Shared filesystem
- Agents write to a shared NFS/SMB directory
- Poll or use fswatch for changes
- Dead simple, no networking code
- **Best for simplicity, worst for latency**

### Recommendation: Option B (HTTP/SSE)
Since we're on separate machines, HTTP is the natural fit. Each agent runs a tiny
HTTP server (or piggybacks on the OpenClaw gateway webhook). Messages are POST'd,
responses are returned synchronously for RPC or streamed via SSE for events.

Could even piggyback on OpenClaw's existing webhook infrastructure — each gateway
already listens on a port. Add a `/agent-comms` endpoint.

## Authentication

- Pre-shared key (PSK) exchanged during initial setup
- Or: derive from a shared secret in both agents' Bitwarden vaults
- All messages signed with HMAC-SHA256
- Optional: mTLS for the HTTP transport (overkill for LAN but future-proof)

## Context Budget Protocol

This is the novel part. LLMs waste tokens on unnecessary context. ACP should let
agents negotiate how much context to exchange:

1. **Requester declares budget**: "I need a minimal answer" vs "give me everything"
2. **Responder compresses accordingly**: If budget is minimal, return structured data only. If full, include reasoning, alternatives, caveats.
3. **Progressive disclosure**: Start minimal, requester can ask for elaboration if needed.

This is fundamentally different from human protocols where you always send the full
message. For LLMs, every token costs money and context window space.

## Shared State Schema

Things both agents should stay in sync on:
```yaml
family:
  members:
    hans: { location, status, calendar_summary }
    grace: { location, status, preferences }
    lavinia: { age: 12, school, activities }
    helene: { age: 8, school, activities }
    walter: { age: 5, school, activities }
  
  calendar:
    today: [ ...events... ]
    tomorrow: [ ...events... ]
    thisWeek: [ ...events... ]

  logistics:
    groceries: [ ...list... ]
    appointments: [ ...upcoming... ]
    travel: { ...current trip info... }

  preferences:
    dinner: { ...dietary notes, favorites... }
    bedtimes: { lavinia: "9pm", helene: "8:30pm", walter: "8pm" }
```

## Implementation Plan

### Phase 1: Manual Bridge (now)
- Agents communicate via iMessage (already works)
- Establish shared state conventions in a shared file (NFS or git repo)
- Prove the patterns before building infrastructure

### Phase 2: HTTP RPC (soon)
- Tiny HTTP server on each agent's machine
- POST /acp/query, POST /acp/delegate, POST /acp/notify
- PSK auth
- JSON envelope format as defined above

### Phase 3: OpenClaw Integration (later)
- Propose as an OpenClaw skill or upstream feature
- `/agent-comms` endpoint on the gateway
- Config-driven agent discovery (agents know each other's endpoints)
- Could become a standard for the OpenClaw ecosystem

## Open Questions

1. Should agents share a memory store (e.g., shared Graphiti instance) or keep separate memories with sync?
2. How to handle conflicting information? (Kit says Hans is at gym, Agent Two says Hans is at dinner)
3. Should the protocol support multi-agent broadcast? (Future: 3+ agents)
4. How much personality should bleed through? Should agent-to-agent messages be purely structured, or is some natural language useful for ambiguous queries?
5. Can we piggyback on OpenClaw's existing webhook/gateway infrastructure instead of building a separate server?
