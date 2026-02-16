# IPC v2 Change Summary

This document summarizes the changes introduced by IPC v2. For the full normative specification, see [`spec/IPC.md`](../spec/IPC.md).

## What IPC v2 Adds

- **`hello` handshake** — Explicit version negotiation per connection. Enables feature discovery and consumer naming.
- **Authentication** — Peer-credential (UID) auth on Linux/macOS, with token-file fallback. Required for v2 connections before most commands.
- **`req_id` correlation** — Every v2 command includes a `req_id` string, echoed in the response. Enables reliable demultiplexing when subscriptions push interleaved events.
- **Receive buffer** — Inbound messages are buffered (default: 1000 messages, 4 MB cap, 24h TTL) so clients can retrieve them after reconnecting.
- **`inbox` / `ack`** — Pull-based retrieval and per-consumer cursor advancement.
- **`subscribe`** — Streaming push with optional replay of buffered messages. Supports kind filtering.
- **`whoami`** — Identity query returning agent ID, public key, and daemon version.
- **Hardened mode** (`ipc.allow_v1 = false`) — Rejects v1 (no-hello) connections entirely.

## Backward Compatibility

- v1 clients (no `hello`) continue to work with legacy broadcast semantics.
- v2 is opt-in per connection via the `hello` handshake.
- When `ipc.allow_v1 = false`, only v2+ clients are accepted.

## Wire-Visible Changes

- **Error codes** are now machine-readable snake_case strings (e.g., `"peer_not_found"`) instead of free-form descriptions.
- **v2 inbound events** use `{"event": "inbound", "replay": false, "seq": N, ...}` instead of `{"inbound": true, "envelope": {...}}`.

## Documentation

- Normative spec: [`spec/IPC.md`](../spec/IPC.md)
- Wire format: [`spec/WIRE_FORMAT.md`](../spec/WIRE_FORMAT.md) §10
- Evaluation rubrics: [`rubrics/`](../rubrics/)
