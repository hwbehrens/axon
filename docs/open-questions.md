# Open Questions

Status: Draft

## Format

Each entry: ID, date opened, context, resolution path, owner, status (open/resolved).

---

## Q-001: Rendezvous server for non-LAN discovery

- Date opened: 2025-01-01
- Context: `spec/SPEC.md` §2 mentions a future rendezvous server for discovery beyond LAN/VPN, but no design exists.
- Date resolved: 2026-08-24
- Resolution: Automatic discovery is local-link only, using Bonjour/zeroconf through DNS-SD/mDNS at `_axon._udp.local.`. AXON will not define a rendezvous, STUN, or WAN-discovery protocol. Explicit peer locators over Tailscale, another VPN, or operator-managed DNS remain supported without becoming discovery mechanisms.
- Follow-up: Remove the future-rendezvous promise from normative specifications. Keep discovery inputs behind a narrow observation boundary without adding speculative rendezvous abstractions.
- Owner: protocol
- Status: resolved

## Q-002: Canonical Agent ID digest length

- Date opened: 2026-08-24
- Context: `spec/SPEC.md` and `spec/WIRE_FORMAT.md` require `ed25519.` plus the first 16 bytes of SHA-256 (32 hex characters), matching the implementation and interoperability tests. Normative `docs/decision-log.md` DEC-002 instead says the full 256-bit digest is used without truncation. Both cannot define the canonical Agent ID format.
- Date resolved: 2026-08-24
- Resolution: The canonical Agent ID is `ed25519.` followed by 32 lowercase hexadecimal characters representing the first 16 bytes of SHA-256 over the Ed25519 public key. This 128-bit fingerprint is the stable cryptographic identifier; it is not a network address. Display names are human-facing labels, and host/port values are mutable locators. The `ed25519` tag identifies the derivation scheme. A new identity key creates a new Agent ID rather than rotating a key beneath an existing ID.
- Follow-up: Correct DEC-002 and make identity, discovery, TLS, peer-token, CLI, and interoperability documentation consistently describe the existing 128-bit format.
- Owner: protocol, identity
- Status: resolved

## Q-003: Canonical IPC peer shape and command inventory

- Date opened: 2026-08-24
- Context: `spec/IPC.md` defines five commands including `add_peer` and requires `agent_id` in `peers` responses. `spec/WIRE_FORMAT.md` §10 lists four commands, omits `add_peer`, and uses `id` in its `peers` response example. Both documents are normative and describe the same local IPC protocol.
- Date resolved: 2026-08-24
- Resolution: Retain the user-scoped Unix-domain-socket IPC boundary and its bounded line-delimited JSON encoding. A separate daemon needs this small, language-neutral local API for agent applications and the CLI. `spec/IPC.md` is the sole detailed IPC authority; `spec/WIRE_FORMAT.md` must reference it instead of duplicating the local contract. The canonical peer field is `agent_id`. The command inventory is `send`, `peers`, `status`, `whoami`, `add_peer`, `remove_peer`, `serve`, and `reply`. `add_peer` and `remove_peer` are the intentional trust lifecycle; `serve` and `reply` implement the single inbound-handler model in Q-004.
- Follow-up: Reconcile the normative documents and add conformance coverage for the complete command and event inventory.
- Owner: protocol, ipc
- Status: resolved

## Q-004: Inbound request ownership and reply mechanism

- Date opened: 2026-08-24
- Context: The protocol promises one `response` or `error` for every bidirectional `request`, and describes an application handler that may answer or decline. IPC only broadcasts the inbound envelope and defines no registration, claim, or reply command, so a local application cannot become that handler or answer on the originating QUIC stream. With multiple IPC clients, ownership and exactly-one-reply behavior are also undefined.
- Date resolved: 2026-08-24
- Resolution: AXON supports application-handled inbound requests through exactly one connection-bound IPC handler lease per daemon identity. `serve` acquires the lease; a competing registration is rejected until the holder disconnects. Requests are delivered only to that handler, while ordinary inbound messages may still be broadcast to observers. `reply` resolves one pending request on its originating QUIC stream. No handler produces an immediate `unhandled` error; handler disconnection terminates its pending requests; duplicate and late replies are rejected; bounded delivery failure produces an explicit error. AXON guarantees at most one protocol reply, not exactly-once application execution or durable request processing.
- Follow-up: Introduce a daemon-owned request broker as the single owner of the handler lease, pending-request map, deadlines, and reply admission. IPC remains transport plumbing and must not independently own request state.
- Owner: protocol, ipc, transport
- Status: resolved

## Q-005: TOFU pin continuity after discovery

- Date opened: 2026-08-24
- Context: A first mDNS observation is admitted as a TOFU pin, but a later mDNS observation for the same Agent ID can replace the discovered public key in `PeerTable`. Cached and static key changes are rejected. The specifications acknowledge first-discovery TOFU but do not say whether unauthenticated discovery may rotate an established dynamic pin.
- Date resolved: 2026-08-24
- Resolution: Once a peer is explicitly enrolled, its validated `(Agent ID, public key)` binding is durable and cannot be changed by discovery. A conflicting public key for the same Agent ID is rejected and surfaced; it never replaces the pin. The same key may acquire, refresh, and lose endpoints without changing trust. Legitimate key rotation creates a new Agent ID and requires explicit enrollment. Durable pin continuity protects later sessions; it does not make an unauthenticated Bonjour observation trustworthy.
- Follow-up: Route discovery observations and runtime enrollment through `PeerDirectory`, publish TLS pins only after durable enrollment, and cover conflicting observations in tests.
- Owner: protocol, discovery, security
- Status: resolved

## Q-006: Duplicate QUIC connection selection

- Date opened: 2026-08-24
- Context: Either peer may initiate a QUIC connection, and simultaneous dialing can create multiple authenticated connections for one peer. The current last-registration-wins map replacement does not close the displaced connection, while the specifications define no deterministic winner or loser shutdown rule.
- Date resolved: 2026-08-24
- Resolution: Each peer has exactly one authoritative connection slot. The policy is "first healthy connection wins within a generation; a failed or timed-out exchange starts a new generation." A healthy incumbent rejects and closes duplicates. Failure invalidates the suspect slot, so retrying redials instead of reusing it. For simultaneous cross-dials, the lexicographically lower Agent ID is the preferred initiator, so both participants prefer the same physical connection. This direction is a tie-breaker only and does not prevent either side from dialing an empty slot.
- Follow-up: Version attempts, candidates, outcomes, and teardown by generation; ensure every loser is closed and joined; ensure stale outcomes cannot clear a newer winner.
- Owner: protocol, transport
- Status: resolved

## Q-007: Dynamic cache trust and address lifetime

- Date opened: 2026-08-24
- Context: `known_peers.json` currently combines a durable public-key pin with a restart address hint. Save time is written as `last_seen_unix_ms`, cached records reset their age on load, and cached peers never expire. The specifications do not define independent trust and address freshness, cache cardinality, or whether IPC `add_peer` is durable outside the CLI workflow.
- Date resolved: 2026-08-24
- Resolution: Separate durable intent from ephemeral observation. `PeerDirectory` is the sole logical owner of validated peer identities, trust pins, configured locators, and live discovery observations. `ConnectionManager` solely owns connection slots, attempts, generations, backoff, QUIC handles, and tasks. `RequestBroker` owns IPC handler and pending-request state. Immutable pinning snapshots, dial targets, and peer views are derived from those owners. Persist only durable pins and user-configured hostname locators; do not persist mDNS-resolved socket addresses, liveness, RTT, backoff, or connection status. Retain configured hostnames and resolve them on each new connection attempt. Address conflicts invalidate or quarantine locator observations and never evict a trusted identity.
- Follow-up: Use an atomic, bounded peer-store adapter only through `PeerDirectory`; publish immutable TLS pinning snapshots; make discovery loss observation-scoped; keep the daemon as sole runtime writer. Q-010 selects the single durable input.
- Owner: protocol, peer directory, persistence
- Status: resolved

## Q-008: Unknown message-kind preservation

- Date opened: 2026-08-24
- Context: `spec/SPEC.md` says unknown kind strings are preserved for forwarding, while the implementation deserializes every unknown string to one `MessageKind::Unknown` sentinel and therefore loses the original value when reserialized. Other normative summaries describe exactly four protocol kinds without clarifying whether the original future string must survive.
- Date resolved: 2026-08-24
- Resolution: There remain exactly four known v1 kinds: `request`, `response`, `message`, and `error`. Receivers retain any unknown remote kind string losslessly. An unknown unidirectional message may be exposed or forwarded unchanged; an unknown bidirectional kind receives `unsupported_kind` because AXON cannot infer its response semantics. Unknown IPC command names are different: they are rejected immediately and are never retained for execution after an upgrade. AXON has no durable inbox, so preservation provides rolling-version forwarding and diagnostics, not retroactive execution.
- Update (2026-02-17, DEC-026): `describe` was added as the fifth known kind. The lossless unknown-kind retention rule and the `unsupported_kind` bidirectional behavior are unchanged; older peers receiving `describe` therefore degrade gracefully.
- Follow-up: Use a lossless unknown-kind representation and add serialization, forwarding, fuzz, and interoperability tests. Keep unknown IPC-command rejection explicit.
- Owner: protocol, message
- Status: resolved

## Q-009: Default admission policy for Bonjour-discovered peers

- Date opened: 2026-08-24
- Context: Bonjour/mDNS is useful for zero-configuration discovery but is unauthenticated. Automatically inserting every advertisement into the TLS pin set makes discovery equivalent to trust admission: any process on the LAN can become an accepted peer by winning first observation. Treating advertisements only as candidates is safer but adds an explicit pairing step to the local-first experience.
- Date resolved: 2026-08-24
- Resolution: Bonjour discovers candidates but never admits them automatically. An explicit local `add_peer` action, using either a currently observed candidate or a peer token, is required before the identity enters the TLS pin set. This small interaction is an intentional guardrail: discovery establishes reachability hints, while a local agent or operator establishes trust. The redesigned v1 has no automatic-TOFU admission mode; one can be proposed later only if a concrete trusted-LAN use case justifies the additional policy surface. Discovery must still validate that Agent ID derives from the advertised public key, and Q-005 pin continuity applies after admission.
- Follow-up: Represent discovered candidates separately from enrolled peers, expose bounded candidate views through IPC, and route enrollment through `PeerDirectory` so no discovery path can mutate trusted state directly.
- Owner: product, protocol, discovery, security
- Status: resolved

## Q-010: Canonical persistent source for enrolled peers

- Date opened: 2026-08-24
- Context: Static peers are currently declarative entries in `config.yaml`, while discovered and runtime-added peers flow through `known_peers.json`. The Q-007 design needs one runtime authority, but persistence can either retain these two operator-facing sources with explicit provenance or migrate all enrolled peers and configured locators into one peer store.
- Date resolved: 2026-08-24
- Resolution: Use one canonical, daemon-owned `peers.json` file for enrolled identities and user-configured hostname locators. The file is a small, versioned, bounded, human- and agent-inspectable JSON document rewritten atomically as a whole; a database, append log, ORM, and migration framework are unnecessary for LAN-scale peer counts. `config.yaml` retains only local daemon settings. Remove peer entries from `config.yaml` and replace `known_peers.json`; do not retain a compatibility or automatic migration path. Legacy peer state is detected and reported with a clear re-enrollment instruction rather than silently imported or ignored. While the daemon is running, the CLI mutates peer state only through IPC so `PeerDirectory` remains the sole live authority.
- Follow-up: Specify the `peers.json` schema and bound, implement crash-safe temporary-write/sync/rename persistence, make `PeerDirectory` the only caller of the store adapter, and remove the replaced config/cache paths completely.
- Owner: product, config, peer directory, persistence
- Status: resolved
