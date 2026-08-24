# Decision Log

Status: Normative

## Format

Each entry: ID, date, subsystem, one-paragraph summary covering motivation, decision, and impact.

## Quick reference

| ID | Date | Subsystem | Title |
|---|---|---|---|
| DEC-014 | 2026-08-24 | transport | Generation-safe authoritative connection selection |
| DEC-013 | 2026-08-24 | ipc, transport | Single connection-bound application request handler |
| DEC-012 | 2026-08-24 | discovery, peer directory, persistence | Intentional LAN admission with one peer authority |
| DEC-011 | 2026-03-13 | rubrics | Adopt shared evaluation infrastructure and agent-readability rubric |
| DEC-010 | 2026-03-13 | repo | Adopt machine-readable agent index and nested AGENTS guidance |
| DEC-009 | 2026-03-13 | docs | Adopt document authority and institutional memory workflow |
| DEC-008 | 2025-01-01 | message | Fixed message kinds at protocol level |
| DEC-007 | 2025-01-01 | ipc | Bounded IPC queues with overflow-disconnect |
| DEC-006 | 2025-01-01 | identity | Base64-encoded identity.key format |
| DEC-005 | 2025-01-01 | ipc, transport | Peer pinning model — reject unknown at TLS |
| DEC-004 | 2025-01-01 | discovery | mDNS for LAN discovery |
| DEC-003 | 2025-01-01 | transport | QUIC over TCP |
| DEC-002 | 2025-01-01 | identity | SHA-256 for agent ID derivation |
| DEC-001 | 2025-01-01 | identity | Ed25519 for identity key pair |

---

## Entries

### DEC-014: Generation-safe authoritative connection selection

Date: 2026-08-24 | Subsystem: transport

Each enrolled peer has one authoritative connection slot owned by `ConnectionManager`. A healthy incumbent wins within its generation; failure, an unhealthy transition, or a failed/timed-out exchange invalidates the suspect slot and advances the generation, so retrying naturally redials. Stale outcomes cannot mutate a newer slot. Simultaneous cross-dials prefer the connection initiated by the lexicographically lower canonical Agent ID so both peers select the same candidate. Every losing or superseded candidate is closed and deterministically reclaimed. This combines stable duplicate suppression with intentional redial recovery instead of relying on unconditional first- or last-registration-wins behavior.

### DEC-013: Single connection-bound application request handler

Date: 2026-08-24 | Subsystem: ipc, transport

AXON retains its small same-user Unix-socket API and supports application-handled requests through exactly one connection-bound handler lease. `RequestBroker`, not IPC or QUIC plumbing, owns the lease, bounded pending-request map, deadlines, and at-most-one reply admission. Ordinary inbound messages remain bounded broadcasts; requests go only to the handler through `serve` and return through `reply`. No handler, disconnect, timeout, overflow, duplicate reply, and late reply are explicit outcomes. `spec/IPC.md` is the sole detailed local-protocol authority, and AXON intentionally does not provide durable request execution or multi-handler routing.

### DEC-012: Intentional LAN admission with one peer authority

Date: 2026-08-24 | Subsystem: discovery, peer directory, persistence

Bonjour/DNS-SD is local-link discovery only and produces validated but untrusted candidates; an explicit same-user `add_peer` action is required before a key enters the TLS pin set. `PeerDirectory` is the sole logical owner of enrolled identities, configured locators, and live observations, deriving immutable pinning and query views; `ConnectionManager` separately owns physical connection state. Durable peer intent lives in one bounded, versioned, atomically rewritten `peers.json` file. `config.yaml` holds local daemon settings only, mDNS addresses and liveness are not persisted, no database or migration framework is introduced, and unsupported `config.yaml` peer entries or `known_peers.json` state require explicit re-enrollment.

### DEC-011: Adopt shared evaluation infrastructure and agent-readability rubric

Date: 2026-03-13 | Subsystem: rubrics

The rubric suite now shares a common set of evaluation principles, a concern-ownership map, explicit spec-to-rubric traceability, and a dedicated agent-readability rubric. This phase turns evaluation guidance into maintained infrastructure rather than repeated prose, reduces double-deduction risk across reviews, and makes repository operability for LLM agents a first-class scored concern.

### DEC-010: Adopt machine-readable agent index and nested AGENTS guidance

Date: 2026-03-13 | Subsystem: repo

The repository now maintains `docs/agent-index.json` plus a complete set of nested `AGENTS.md` files for every major subsystem. This keeps the root onboarding document dense while giving agents deterministic task routing and local guardrails once they enter a subsystem. Future module additions or renames must update both the agent index and nested guidance in the same change to avoid drift.

### DEC-009: Adopt document authority and institutional memory workflow

Date: 2026-03-13 | Subsystem: docs

AXON now treats document status, authority ordering, escalation behavior, the decision log, and open questions as maintained project infrastructure rather than informal conventions. This phase makes spec conflicts and unresolved ambiguities explicit, gives contributors a single place to record architectural decisions, and establishes the documentation model that the later agent-index and rubric phases build on.

### DEC-008: Fixed message kinds at protocol level

Date: 2025-01-01 | Subsystem: message

The protocol defines exactly 4 known message kinds (`request`, `response`, `message`, `error`) at the wire level. New application-level semantics are expressed via message payload content, not new kinds. Receivers nevertheless retain an unrecognized kind's exact string: unknown unidirectional messages may be exposed or forwarded unchanged, while unknown bidirectional kinds receive `unsupported_kind`. This keeps known semantics minimal without destroying rolling-version information. Adding a known kind requires a spec update to `spec/MESSAGE_TYPES.md`.

### DEC-007: Bounded IPC queues with overflow-disconnect

Date: 2025-01-01 | Subsystem: ipc

Per-IPC-client outbound message queues are bounded (`MAX_CLIENT_QUEUE = 1024`). When a client lags behind and the queue overflows, the daemon disconnects the client rather than silently dropping messages. This preserves message ordering guarantees — a client either sees all messages in order or gets disconnected and can reconnect. Silent drop was rejected because it creates invisible data loss that LLM agents cannot detect or recover from.

### DEC-006: Base64-encoded identity.key format

Date: 2025-01-01 | Subsystem: identity

The `identity.key` file stores a base64-encoded 32-byte Ed25519 seed. Raw binary format was used in an earlier version but rejected because: (1) it's not inspectable by agents or humans, (2) it's ambiguous whether the file contains a seed or full keypair, and (3) base64 is a safe, portable text encoding. Non-base64 or raw legacy formats are rejected at load time; `axon doctor --fix --rekey` can regenerate.

### DEC-005: Peer pinning model — reject unknown at TLS

Date: 2025-01-01 | Subsystem: ipc, transport

Unknown peers are rejected during TLS handshake, not after. Peers must be explicitly enrolled in `PeerDirectory` before their key appears in the immutable pinning snapshot read by TLS verifiers; a Bonjour observation alone is insufficient. This is a zero-trust-by-default posture: the daemon never processes messages from unauthenticated or merely discovered peers. Once admitted, a conflicting key cannot replace the pin; legitimate rekeying creates a new Agent ID and requires explicit enrollment.

### DEC-004: mDNS for LAN discovery

Date: 2025-01-01 | Subsystem: discovery

mDNS/DNS-SD (`_axon._udp.local.`) is the discovery mechanism for LAN deployments. It requires no address configuration, but advertisements are untrusted candidates and do not grant admission. Explicit peer tokens provide DNS or VPN/Tailscale locators when mDNS is unavailable. Automatic discovery is deliberately local-link only; AXON does not define a rendezvous, STUN, or WAN-discovery service.

### DEC-003: QUIC over TCP

Date: 2025-01-01 | Subsystem: transport

QUIC was chosen over TCP for transport because: (1) multiplexed streams avoid head-of-line blocking — a slow response doesn't block fire-and-forget messages, (2) TLS 1.3 is built into the protocol — no separate TLS handshake layer, (3) connection migration supports agents that change network addresses, and (4) the `quinn` crate provides a mature Rust implementation. The overhead of UDP/QUIC vs TCP is negligible for AXON's message sizes.

### DEC-002: SHA-256 for agent ID derivation

Date: 2025-01-01 | Subsystem: identity

Agent ID is derived from the first 16 bytes of `SHA-256(Ed25519_public_key)`, formatted as `ed25519.` plus 32 lowercase hexadecimal characters. The 128-bit fingerprint is deterministic, compact, and commensurate with Ed25519's security level for AXON's bounded peer population; the `ed25519.` prefix makes the derivation scheme explicit. Agent ID is a cryptographic identifier used in IPC, mDNS TXT records, and TLS SNI, not a mutable host/port locator.

### DEC-001: Ed25519 for identity key pair

Date: 2025-01-01 | Subsystem: identity

Ed25519 was chosen for agent identity because: (1) single-purpose signing key — no accidental misuse as encryption key, (2) fast key generation and verification for a daemon that may restart frequently, (3) compact keys (32-byte seed, 32-byte public key), and (4) wide ecosystem support via `ed25519-dalek`. RSA was rejected for key size; ECDSA (P-256) was rejected for implementation complexity and historical vulnerabilities.
