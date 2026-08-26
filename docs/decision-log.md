# Decision Log

Status: Normative

## Format

Each entry: ID, date, subsystem, one-paragraph summary covering motivation, decision, and impact.

## Quick reference

| ID | Date | Subsystem | Title |
|---|---|---|---|
| DEC-020 | 2026-08-24 | transport, daemon, ipc, broker, discovery | Round-five review hardening: absolute deadlines, enrollment epochs, peer-scoped correlation |
| DEC-019 | 2026-08-24 | transport, broker, discovery, persistence | Whole-exchange deadlines, same-call tombstones, bounded tracking |
| DEC-018 | 2026-08-24 | transport, request broker, ipc | Deadline-owned sends, tombstoned terminal outcomes, shutdown liveness |
| DEC-017 | 2026-08-24 | transport, daemon, ipc | Revocation-linearized admission, at-most-once retries, bounded drains |
| DEC-016 | 2026-08-24 | transport | Single retry on a send that fails during cross-dial convergence |
| DEC-015 | 2026-08-24 | peer directory, testing | Adopt Hegel for stateful property testing of the peer directory |
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

### DEC-020: Round-five review hardening: absolute deadlines, enrollment epochs, peer-scoped correlation

Date: 2026-08-24 | Subsystem: transport, daemon, ipc, request broker, discovery

Review of the connection-ownership redesign surfaced four error classes that this decision closes class-wide rather than patch-by-patch. **Absolute deadlines:** every await in the send path (`connect_peer`, the per-peer dial lock, stream open, frame write, response read) now recomputes the remaining budget from one `Instant` deadline, so a caller's N-second exchange can never consume more than N seconds in total; hostile `timeout_secs` values are bounded at IPC (`MAX_REQUEST_TIMEOUT_SECS = 3600`) and overflow-checked at the transport, and the reserved send slot is held by an unwind-safe RAII guard so a panic can no longer permanently leak capacity. **Authority-race admission:** handshakes capture an enrollment epoch (bumped on every revocation) before they begin and admission re-checks it under the registry lock alongside current enrollment, so a pre-revocation outbound handshake or untracked inbound handshake can never be admitted against re-enrolled trust — a trusted attempt must start after revocation committed; canceled reconnect attempts additionally release their `ReconnectBook` ticket via `abandoned` instead of staying `in_flight` forever. **Selection-rule precision:** direction is now strictly a simultaneous-cross-dial tie-breaker — a preferred-direction candidate may replace a healthy incumbent only within one `DIAL_TIMEOUT` of the incumbent's installation (the window in which a genuine racing handshake can still arrive); afterwards the healthy incumbent wins per SPEC.md §Connection Lifecycle 4. **Principal-scoped correlation:** inbound request correlation (pending entries, terminal-outcome tombstones, completed-response cache) is keyed by `(authenticated remote AgentId, request UUID)` so one peer's cached outcome can never be replayed to or satisfy a reply for another peer's exchange; the IPC `reply` command gains an optional `peer` field and uuid collisions without it fail explicitly as ambiguous. Finally, mDNS service-tracking eviction now evicts the oldest-INSERTED live service via explicit insertion-order tracking, matching its documented contract.

### DEC-019: Whole-exchange deadlines, same-call tombstones, bounded tracking

Date: 2026-08-24 | Subsystem: transport, request broker, discovery, persistence

Fourth-round review closed five residual gaps. The send deadline now covers every phase of an exchange: DNS resolution, `open_uni`/`open_bi` stream opens (peer-controlled stream credits could otherwise stall a send past `timeout_secs`), and request frame writes are each bounded by the remaining budget, alongside the existing handshake, response-wait, and uni-write bounds. `begin()` rechecks the completed cache after its lazy sweep, so a retry arriving in the same call that sweeps its own prior attempt replays the tombstoned terminal outcome instead of becoming a fresh delivery the stale attempt's late reply could satisfy. `reply` validates the encoded envelope against the 65,536-byte wire limit BEFORE consuming the pending request, so oversized replies are rejected at IPC (`invalid_command`) instead of being acknowledged locally and silently dropped by transport framing. Peer-store saves enforce the same byte cap loads enforce (and `decode` rejects oversize input before parsing), so enrollment can no longer produce a store the next restart refuses to load. `close_peer` cancels a per-peer dial token observed by handshakes, address iteration, and reconnect tasks, satisfying the IPC revocation contract's requirement to cancel in-flight attempts rather than merely refuse their admission. mDNS service tracking is bounded at `MAX_TRACKED_SERVICES` (1024) with oldest-entry eviction that emits lost events, independent of peer-directory limits.

### DEC-018: Deadline-owned sends, tombstoned terminal outcomes, shutdown liveness

Date: 2026-08-24 | Subsystem: transport, request broker, ipc

Third-round review found that the IPC layer's outer `tokio::time::timeout` around `send_to` could drop the send future mid-flight, skipping the precise connection retirement and leaving a stale slot registered. The budget is now owned entirely inside the transport: `send_to` derives a deadline and bounds every phase (per-dial handshake via `DIAL_TIMEOUT`, frame writes, response wait), so no external canceller exists and every failure returns normally through retirement; `SendError::timed_out` preserves the spec contract that request timeouts surface as `timeout`, never `peer_unreachable`. In `RequestBroker`, the completed-response replay check moved before the handler lookup, and every terminal outcome (lazy-sweep expiry, `fail`, handler disconnect, reconcile) is tombstoned into the bounded completed cache: a retried exchange after handler loss replays its recorded outcome instead of reporting `unhandled`, and a swept request UUID can never be redelivered for a stale handler's late reply to satisfy. Shutdown liveness: IPC client handlers use cancellation-aware command-channel sends so shutdown cannot strand a task blocked on a full channel, and reconnect dial tasks observe the transport's cancel token so `close_all`'s join window covers every tracked task. The Makefile lint gate now runs clippy over all targets so test-only lints fail locally exactly as they would in review.

### DEC-017: Revocation-linearized admission, at-most-once retries, bounded drains

Date: 2026-08-24 | Subsystem: transport, daemon, ipc

Second-round review of the transport/IPC concurrency surface found three races and two contract gaps. (1) Connection admission now consults `PeerDirectory::is_enrolled` inside a gate run while holding the registry's admission lock (`ConnectionRegistry::admit_gated`), on both inbound and outbound paths. Because revocation's `close_peer` takes that same lock after committing the directory change, a handshake racing `remove_peer` either fails the gate or is closed immediately after installation — it can never land a live slot. Lock ordering is registry-state → directory-state; no reverse path exists. (2) The send retry path retires exactly the failed exchange's slot via `retire_if_current_connection`; the wholesale `close_peer` calls in `send_to` and the IPC request-timeout branch were removed because they could destroy a healthy replacement installed concurrently. Additionally, retry eligibility is now classified by delivery ambiguity: fire-and-forget kinds are retried only when the failure occurred before any payload byte was written (`SendError::ambiguous == false`), preserving at-most-once application delivery; requests keep DEC-016's single retry per their documented at-most-one-reply semantics. (3) The IPC overlong-line drain — needed so the queued `command_too_large` error survives RST — is bounded by `IPC_OVERLONG_DRAIN_TIMEOUT` (2s) and cancellation-aware, so a pausing client cannot hold an IPC client slot indefinitely. Peer-store loading refuses non-regular files (symlinks/FIFOs/devices) via `symlink_metadata` before opening and caps reads at `MAX_PEER_STORE_BYTES` (1 MiB). Send-capacity accounting moved into the command handler as a compare-and-swap reservation, so `MAX_INFLIGHT_SENDS = N` admits exactly N concurrent sends.

### DEC-016: Single retry on a send that fails during cross-dial convergence

Date: 2026-08-24 | Subsystem: transport

When both peers dial during startup, a send can land on the connection that loses DEC-014's tie-break; the winner closes it and the exchange fails with a framing error even though a healthy authoritative slot exists milliseconds later. `send_to` now treats such a failure as Q-006's suspect-slot case: it advances the generation by closing the failed slot, redials, and retries the exchange exactly once against the refreshed slot. One retry only — repeated failures still surface as `peer_unreachable`, and AXON's documented at-most-once application-execution guarantee makes the duplicate-delivery risk of a single transport-level retry acceptable. Genuine outages pay one extra refused dial before erroring. Refined by DEC-017: the blanket retry violated at-most-once for fire-and-forget kinds; retries are now gated on delivery-ambiguity classification.

### DEC-015: Adopt Hegel for stateful property testing of the peer directory

Date: 2026-08-24 | Subsystem: peer directory, testing

Peer-directory trust invariants are stateful properties (rules applied to live state, invariants checked after each transition), and the previous hand-rolled proptest op list shrank counterexamples poorly. The `hegeltest` crate (Hegel-rust, MIT, beta) provides Hypothesis-style rule-based state machines with high-quality shrinking, so `peer_directory/state_machine_tests.rs` now expresses observe/enroll/revoke rules and the four trust/durability invariants declaratively; the proptest store roundtrip and other value-level proptests remain on proptest, which stays the repo default where statefulness does not earn Hegel's cost. Adoption is deliberately narrow (one dev-dependency use site, pinned major version) because Hegel is beta and may make breaking changes; revisit scope only if another subsystem has genuinely stateful property needs. `HEGEL_TEST_CASES` scales coverage without code changes for nightly deep runs.

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
