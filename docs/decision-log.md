# Decision Log

Status: Normative

## Format

Each entry: ID, date, subsystem, one-paragraph summary covering motivation, decision, and impact.

## Quick reference

| ID | Date | Subsystem | Title |
|---|---|---|---|
| DEC-026 | 2026-02-17 | message, manifest, request_broker, ipc, daemon | Capability manifests: daemon-answered `describe`, `serve`-time publication, `who_can` derived view |
| DEC-025 | 2026-09-01 | transport, daemon | Structural revocation pairing (`revoke_peer`) and fail-closed epoch-lock poisoning |
| DEC-024 | 2026-08-26 | transport, peer_directory, ipc, daemon | Maintainability simplification: synchronous pin-snapshot admission gate, tripwire removal, registry read-path clarity, whoami flattening |
| DEC-023 | 2026-08-26 | peer_directory, ipc, daemon | Round-eight re-review hardening: serialized persistence transactions, bounded req_id, encoder-gated client-handler errors, capacity-modeling Hegel machine |
| DEC-022 | 2026-08-26 | peer_directory, ipc, daemon, transport | Round-seven review hardening: ghost-free revocation, framed outbound IPC, transactional persistence, typed directory errors |
| DEC-021 | 2026-08-24 | transport, peer_directory, discovery | Round-six review hardening: per-peer enrollment epochs, disk-I/O-free directory persistence, fully bounded/cancellable send path |
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
| DEC-003 | 2025-01-01 | transport | QUIC instead of TCP |
| DEC-002 | 2025-01-01 | identity | SHA-256 for agent ID derivation |
| DEC-001 | 2025-01-01 | identity | Ed25519 for identity key pair |

---

## Entries

### DEC-026: Capability manifests — daemon-answered `describe`, `serve`-time publication, `who_can` derived view

Date: 2026-02-17 | Subsystem: message, manifest, request_broker, ipc, daemon

AXON solved identity discovery but not capability discovery: a newly connected agent had a socket and zero vocabulary — the real "shout into the void" happened after a successful connection. Three coupled decisions. **(1) `describe` becomes the fifth known message kind**, answered by the *receiving daemon* from the manifest its local handler published at `serve` time; the handler is never woken. A payload-level `op` convention on `request` was rejected: it would require the daemon to inspect payloads (violating "payloads are opaque") or couple directory queries to handler liveness while forcing every application to re-implement describe boilerplate it already published. The kind count grew deliberately — the four-kind rule predates any use case for capability query, and unknown-kind lossless retention gives graceful degradation (`unsupported_kind` naming the string) against older peers. **(2) Manifests are claims, never authority:** publication is opt-in, absence is explicit (`no_manifest`), and a manifest never affects TLS trust, pinning, or enrollment — only exercising a service validates a claim. The broker answers `describe` before the completed-response tombstone cache, because describe is side-effect free and a fresh answer is always correct across manifest refreshes. **(3) `who_can` is a derived, cached view** over connected enrolled peers only, refreshed by TTL-gated daemon-issued `describe` pulls; it is runtime-only (no durable state, no new ownership domain) and names peers that fail a pull instead of silently omitting them. Referrals deliberately remain peer tokens — no gossip, no transitive trust, no reputation scores. Alternatives considered and rejected: connection-time manifest exchange as framing-layer metadata (touches WIRE_FORMAT framing and ConnectionManager generation logic for little gain over pull) and mandatory blocking hellos on every connection (a handshake tax; ALPN already negotiates the protocol version).

### DEC-025: Structural revocation pairing (`revoke_peer`) and fail-closed epoch-lock poisoning

Two Low findings from the swarm review of DEC-024's simplification pass, hardened without changing protocol behavior; superseded in part by the post-review fix round below. **(1) Structural revocation pairing:** the revocation guarantee — either the admission gate refuses a handshake that raced the revocation, or the subsequent `close_peer` tears the freshly installed slot down — previously depended on every caller pairing `PeerDirectory::remove_peer` with `ConnectionManager::close_peer`, enforced only by convention at the single production call site. `ConnectionManager::revoke_peer` now performs the paired sequence (directory commit, then teardown) and the daemon's `remove_peer` IPC command is its only caller. On a failed commit (peer not enrolled, persistence error) nothing is torn down: transport authority follows trust, never leads it. **(2) Fail-closed epoch poisoning:** every enrollment-epoch access previously panicked via `.expect` on lock poison — including inside the admission gate while it holds the registry write lock, cascading poison to all registry users. Poisoning now fails closed: the gate rejects every admission while the lock is poisoned (the lock is never recovered or cleared), epoch captures default to the never-revoked zero, and `close_peer` skips the bump with a warning. The slot-teardown half of revocation is unaffected, so a revocation racing a poisoned lock still tears down live slots; captures taken during poisoning can never be admitted because the gate reads the same poisoned lock. A poisoned lock is permanent until restart — refusing new connections after unexplained internal corruption is the conservative choice for a daemon whose new connections are security-relevant.

**Post-review fix round (same PR, addressing reviewer P1/P2):** `PeerDirectory::remove_peer` is now `pub(crate)`, so no caller — in-tree or library — can remove trust without the paired teardown; `revoke_peer` is the sole public revocation surface. The pair additionally runs on a task detached from `revoke_peer`'s caller: the directory's persistent edit is itself cancellation-shielded (its transaction worker completes once started), so a caller cancelled between commit and teardown could otherwise strand a live connection against revoked trust. A `JoinError` (runtime shutdown, task panic) surfaces as `DirectoryError::Persist`; a cancellation interleaving test (`cancelled_revoke_still_tears_down_transport`) pins that an aborted caller still lands both halves.

**Second fix round (same PR, addressing the verification lane's findings):** the pair task is spawned through the manager's `TaskTracker` rather than bare `tokio::spawn`, so `close_all` joins in-flight revocations at shutdown instead of dropping them mid-pair; it deliberately does NOT observe shutdown cancellation (once the commit lands, teardown must follow), and `close_all`'s bounded wait drains normal-speed pairs. The determinism gate is a per-manager field (never process-global), eliminating cross-test interference under parallel test execution. The cancellation test now exercises both phases and was verified discriminating by mutation: inlining the pair (the detach bug) fails the caller-abort assertion, and untracked spawning (the tracker bug) fails the close_all-join assertion.

**Third fix round (same PR, replacing the over-claimed shutdown boundary):** an `is_closed()` check before spawning claimed a no-new-revocations boundary but raced `close()` (check-then-spawn TOCTOU). Rather than adding lifecycle-lock machinery, the check is deleted and the guarantee is stated truthfully: a revocation racing shutdown may commit durably after the shutdown wait — safe by construction, because `close_all` closes the endpoint before waiting, so no live connection exists to strand, the directory commit is atomic once started (memory/disk stay consistent), and a post-shutdown `close_peer` is a no-op on an empty registry. The pairing's obligation is defined against live transport; after shutdown there is nothing to tear down. If a hard refusal boundary is ever needed, the upgrade path is a lifecycle lock serializing check/spawn against `close_all`.

### DEC-024: Maintainability simplification: synchronous pin-snapshot admission gate, tripwire removal, registry read-path clarity, whoami flattening

A code review flagged four design-friction spots; all four were simplified without changing any behavioral guarantee. **(1) Synchronous admission gate:** the connection-admission gate previously awaited `PeerDirectory::is_enrolled` while holding the registry's write lock, which required the `ADMISSION_GATE_BUDGET` fail-closed bound, async-gate generic machinery, and a documented (unstructured) lock-ordering rule between the registry and the directory. The gate now reads the published pinning snapshot — the SAME immutable enrollment oracle the TLS verifiers already consume synchronously — plus the per-peer enrollment epoch, both plain std-lock reads, so it runs entirely inside the registry's write-lock critical section with no await. The budget, the async machinery, and `PeerDirectory::is_enrolled` are deleted, and the hazard is structurally impossible rather than documented away. The revocation guarantee is unchanged: pins lag live state only between a commit's apply and its pins publish; in that window a just-enrolled peer is briefly refused (conservative; reconnect maintenance retries within one second) and a just-revoked peer may be briefly admitted, but `remove_peer`'s caller always follows with `close_peer`, which tears the freshly installed slot down — either the gate refuses or the revocation itself closes the slot. **(2) Tripwire removal:** the `persist_generation` counter and its invariant check in `run_persistent_edit` were provably unreachable under DEC-023's fully serialized save gate; the counter, check, and its `Persist` error path are removed. The rest of `PersistPlan` (snapshot → lock-free save → delta apply) is retained deliberately: it is the price for no-disk-I/O-under-the-state-lock and no-clobbering-of-concurrent-observations. **(3) Registry read-path clarity:** the duplicated reap-closed-slot-and-bump-generation logic in `current()` and `admit_gated_with_window` is unified as `RegistryState::reap_closed`, and `current()` is renamed `live_slot` to convey that it is a mutating read (lazy reaping requires the write lock). Lazy reaping stays: a periodic reaper would make sends to a just-died peer waste one exchange. **(4) Whoami flattening:** `Whoami` was dispatched IPC → daemon command loop → back into `IpcServer::handle_command`, a method that handled only Whoami and error-replied to everything else. The daemon command handler now composes the reply from `IpcServer::whoami_info()` and `IpcServer::handle_command` is deleted. Supersedes the admission-gate mechanism described in DEC-017/020/021; the guarantees those entries established (revocation-linearized admission, epoch scoping, generation safety) all hold.

### DEC-023: Round-eight re-review hardening: serialized persistence transactions, bounded req_id, encoder-gated client-handler errors, capacity-modeling Hegel machine

A subagent re-review of DEC-022 confirmed all seven round-seven dispositions but found four new defects, three of them in the round-seven fixes themselves. **Panic-free oversized-reply fallback:** the `message_too_large` fallback preserved the caller's `req_id`, whose echo is unbounded — a legal 65,536-byte command with a ~65KB `req_id` made the fallback itself oversized and its `expect` panicked the daemon. `req_id` is now bounded at ingress (`MAX_REQ_ID_BYTES = 1024`, spec/IPC.md §3: longer values rejected with `invalid_command`, never echoed), and the fallback encodes through `error_reply_line`, which drops the echo rather than truncating or panicking. **Encoder-gated client-handler errors:** malformed-command replies serialized directly with an `expect`, bypassing the shared encoder and echoing unbounded req_ids — a legal-size malformed frame produced a 65KB+ error line on the raw socket. All error lines now pass `error_reply_line`. **Serialized persistence transactions:** round seven's save-then-apply released the save lock between save and apply, leaving a window where a paused committer's fresh save could be overwritten by an older heal snapshot — a bail or crash there left disk older than memory. Worse, the 8-attempt generation-retry budget failed under ordinary 8-way concurrency (an interleaving test produced spurious `Persist` errors). Persistent edits are now ONE fully serialized transaction: the save gate is held across build, save, AND apply, so the generation cannot move between snapshot and apply — no retries, no heal, no divergence windows; the retry loop and heal path are deleted. **Hegel capacity modeling:** the bounds invariants were partly vacuous (six peers vs the 256 bound; four locator seeds vs eight; the observe rule asserted `CapacityReached` never occurs). The machine now models per-peer observation capacity (20 slots across the 16 bound) and draws nine locator seeds across the eight bound, so both rejection branches are reachable and the `directory_bounds_hold` invariant has teeth. Test inventories (`ALL_CODES`, wire-format expected/actual lists) now enumerate `message_too_large`, and `docs/agent-index.json` lists the persistence module.
### DEC-022: Round-seven review hardening: ghost-free revocation, framed outbound IPC, transactional persistence, typed directory errors

Seven findings; six confirmed and fixed, one refuted by test. **Ghost-free revocation:** round-six's `remove_peer` applied its delta with the snapshot's observation IDs, so an `observe` landing between snapshot and commit left its ID in `observation_index` pointing at no record — invisible to expiry, leaking forever. The apply step now removes the record's ENTIRE observation set at commit time; observations arriving after the commit are legitimate post-revocation discovery. Pinned by a deterministic state-level interleaving test, a 25-round concurrent revoke/observe stress test, and a new Hegel invariant (`observation_index_stays_ghost_free`) — bounds and index consistency are now checked for free after every rule, alongside `directory_bounds_hold`. **Framed outbound IPC:** replies, broadcasts, and handler deliveries serialized and enqueued without size checks, so a near-limit network envelope became an oversized IPC line once wrapped in event JSON. All outbound lines now pass one encoder enforcing the 65,536-byte limit including the newline: oversized replies fail explicitly with the new `message_too_large` error (req_id preserved), oversized broadcasts are dropped with a warning, oversized handler deliveries fail so the broker sends the remote requester one terminal error; nothing is ever truncated. The CLI's outbound check was off by one (permitted a 65,537-byte frame) and now counts the newline. **Transactional persistence:** the heal path held a read lock across `store.save`, violating the lock-free persistence invariant; it now snapshots under a read lock, saves under a dedicated save mutex with a generation re-check, and commits with a no-op generation bump. save-then-apply runs on an owned transaction worker, so caller cancellation cannot leave disk ahead of memory; `store.save` treats post-rename directory-sync failures as warnings (an Err always means the file is unchanged), and both classes carry failure-injection/abort tests. **Typed directory errors:** every directory error used to map to `peer_not_found`/`peer_not_observed`, misreporting capacity and persistence failures; `DirectoryError` now carries NotEnrolled/NotObserved/LocalAgentId/EnrolledCapacity/LocatorCapacity/Persist, and the IPC layer maps only the unknown-peer classes to user-facing codes. **Refuted:** "full observation sets reject refreshes" — `observe` withdraws the existing observation ID before the capacity check, so same-ID refreshes at capacity already succeed; pinned by exact-boundary tests proving `observed_at` still advances for both enrolled peers and candidates. (The save-then-apply design with generation retries and the heal path described above was replaced in DEC-023 by one fully serialized transaction per edit.)
### DEC-021: Round-six review hardening: per-peer enrollment epochs, disk-I/O-free directory persistence, fully bounded/cancellable send path

Review of the round-five hardening surfaced four residual classes, closed structurally. **Per-peer enrollment epochs:** the single global epoch bumped on every revocation let revoking peer B reject an otherwise valid in-flight handshake for unrelated peer A; epochs are now tracked per AgentId (bumped only by `close_peer`, never pruned — pruning would allow ABA reuse of epoch 0 against restored trust), inbound handshakes snapshot the full epoch map before the handshake and admission compares only the authenticated peer's entry, so revocation remains linearized per-peer without cross-peer interference. **No disk I/O under the directory lock:** `enroll_candidate`/`enroll`/`remove_peer` previously held the directory write lock across `PeerStore::save`, letting one stalled save block every reader (`dial_targets`, the admission gate) indefinitely; persistence now validates against a read-lock snapshot, saves with no lock held, and commits its delta under a short write lock guarded by a `persist_generation` counter (lost races retry; exhaustion heals the store from live memory before failing). Deltas are applied onto fresh state rather than swapping whole snapshots so concurrent ephemeral observations are never clobbered. (The save-then-apply generation check was later replaced by DEC-023's fully serialized transactions.) **Every send-path await is bounded or cancellable:** peer lookup shares the exchange deadline, DNS resolution selects on the per-peer dial token (abandoned `spawn_blocking` resolvers have their results dropped), the per-peer dial-lock wait selects on cancellation too, and the admission gate's enrollment lookup fails CLOSED under `ADMISSION_GATE_BUDGET` (or the caller's remaining budget) instead of stalling connection admission forever. **mDNS insertion-order boundedness:** a refresh of a known service whose stored set is empty (self/malformed/address-less) re-satisfied `previous.is_empty()`, re-appending to `insertion_order` on every periodic refresh despite the map cap; names are enqueued only when genuinely new, enforced by a testable `ServiceTracker` with repeated-empty-refresh regression coverage.

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

Peer-directory trust invariants are stateful properties (rules applied to live state, invariants checked after each transition), and the previous hand-rolled proptest op list shrank counterexamples poorly. The `hegeltest` crate (Hegel-rust, MIT, beta) provides Hypothesis-style rule-based state machines with high-quality shrinking, so `peer_directory/state_machine.rs` (module `state_machine_tests`) now expresses observe/enroll/revoke rules and the four trust/durability invariants declaratively; the proptest store roundtrip and other value-level proptests remain on proptest, which stays the repo default where statefulness does not earn Hegel's cost. Adoption is deliberately narrow (one dev-dependency use site, pinned major version) because Hegel is beta and may make breaking changes; revisit scope only if another subsystem has genuinely stateful property needs. `HEGEL_TEST_CASES` scales coverage without code changes for nightly deep runs.

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

> Superseded in part by DEC-026 (2026-02-17): `describe` was added as the fifth known kind, with daemon-answered capability semantics. The lossless unknown-kind retention rule below is unchanged.

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

### DEC-003: QUIC instead of TCP

Date: 2025-01-01 | Subsystem: transport

QUIC over UDP was chosen instead of TCP because: (1) multiplexed streams avoid head-of-line blocking — a slow response doesn't block fire-and-forget messages, (2) TLS 1.3 is built into the protocol — no separate TLS handshake layer, (3) connection migration supports agents that change network addresses, and (4) the `quinn` crate provides a mature Rust implementation. The overhead of UDP/QUIC vs TCP is negligible for AXON's message sizes.

### DEC-002: SHA-256 for agent ID derivation

Date: 2025-01-01 | Subsystem: identity

Agent ID is derived from the first 16 bytes of `SHA-256(Ed25519_public_key)`, formatted as `ed25519.` plus 32 lowercase hexadecimal characters. The 128-bit fingerprint is deterministic, compact, and commensurate with Ed25519's security level for AXON's bounded peer population; the `ed25519.` prefix makes the derivation scheme explicit. Agent ID is a cryptographic identifier used in IPC, mDNS TXT records, and TLS SNI, not a mutable host/port locator.

### DEC-001: Ed25519 for identity key pair

Date: 2025-01-01 | Subsystem: identity

Ed25519 was chosen for agent identity because: (1) single-purpose signing key — no accidental misuse as encryption key, (2) fast key generation and verification for a daemon that may restart frequently, (3) compact keys (32-byte seed, 32-byte public key), and (4) wide ecosystem support via `ed25519-dalek`. RSA was rejected for key size; ECDSA (P-256) was rejected for implementation complexity and historical vulnerabilities.
