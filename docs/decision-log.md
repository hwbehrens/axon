# Decision Log

Status: Normative

## Format

Each entry: ID, date, subsystem, one-paragraph summary covering motivation, decision, and impact.

## Quick reference

| ID | Date | Subsystem | Title |
|---|---|---|---|
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

### DEC-008: Fixed message kinds at protocol level

Date: 2025-01-01 | Subsystem: message

The protocol defines exactly 4 message kinds (`request`, `response`, `message`, `error`) at the wire level. New application-level semantics are expressed via message payload content, not new kinds. This keeps the protocol surface minimal and prevents kind-proliferation that would fragment the spec. Adding a new kind requires a spec update to `spec/MESSAGE_TYPES.md`.

### DEC-007: Bounded IPC queues with overflow-disconnect

Date: 2025-01-01 | Subsystem: ipc

Per-IPC-client outbound message queues are bounded (`MAX_CLIENT_QUEUE = 1024`). When a client lags behind and the queue overflows, the daemon disconnects the client rather than silently dropping messages. This preserves message ordering guarantees — a client either sees all messages in order or gets disconnected and can reconnect. Silent drop was rejected because it creates invisible data loss that LLM agents cannot detect or recover from.

### DEC-006: Base64-encoded identity.key format

Date: 2025-01-01 | Subsystem: identity

The `identity.key` file stores a base64-encoded 32-byte Ed25519 seed. Raw binary format was used in an earlier version but rejected because: (1) it's not inspectable by agents or humans, (2) it's ambiguous whether the file contains a seed or full keypair, and (3) base64 is a safe, portable text encoding. Non-base64 or raw legacy formats are rejected at load time; `axon doctor --fix --rekey` can regenerate.

### DEC-005: Peer pinning model — reject unknown at TLS

Date: 2025-01-01 | Subsystem: ipc, transport

Unknown peers are rejected during TLS handshake, not after. Peers must be present in the PeerTable's shared PubkeyMap before a connection is accepted. This is a zero-trust-by-default posture: the daemon never processes messages from unauthenticated peers. The PeerTable is the single source of truth for peer identity; TLS verifiers read from it, eliminating manual sync requirements.

### DEC-004: mDNS for LAN discovery

Date: 2025-01-01 | Subsystem: discovery

mDNS/DNS-SD (`_axon._udp.local.`) is the primary discovery mechanism for LAN deployments. It requires zero configuration — agents on the same network discover each other automatically. For VPN/Tailscale deployments where mDNS isn't available, static peers in `config.yaml` provide an explicit fallback. A future rendezvous server is noted in the spec but deferred.

### DEC-003: QUIC over TCP

Date: 2025-01-01 | Subsystem: transport

QUIC was chosen over TCP for transport because: (1) multiplexed streams avoid head-of-line blocking — a slow response doesn't block fire-and-forget messages, (2) TLS 1.3 is built into the protocol — no separate TLS handshake layer, (3) connection migration supports agents that change network addresses, and (4) the `quinn` crate provides a mature Rust implementation. The overhead of UDP/QUIC vs TCP is negligible for AXON's message sizes.

### DEC-002: SHA-256 for agent ID derivation

Date: 2025-01-01 | Subsystem: identity

Agent ID is derived as `SHA-256(Ed25519_public_key)`, formatted as `ed25519.<hex>`. SHA-256 was chosen because: (1) it's deterministic — same key always produces the same ID, (2) 256-bit output provides collision resistance without truncation, and (3) the `ed25519.` prefix makes the key type explicit and extensible. The agent ID is the canonical peer identifier used in IPC, mDNS TXT records, and TLS SNI.

### DEC-001: Ed25519 for identity key pair

Date: 2025-01-01 | Subsystem: identity

Ed25519 was chosen for agent identity because: (1) single-purpose signing key — no accidental misuse as encryption key, (2) fast key generation and verification for a daemon that may restart frequently, (3) compact keys (32-byte seed, 32-byte public key), and (4) wide ecosystem support via `ed25519-dalek`. RSA was rejected for key size; ECDSA (P-256) was rejected for implementation complexity and historical vulnerabilities.
