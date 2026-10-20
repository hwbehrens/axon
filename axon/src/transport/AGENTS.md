# AGENTS.md (transport)

This file applies to QUIC/TLS transport code in `axon/src/transport/`.

## Priorities

TLS security > protocol correctness > performance.

## File responsibilities

- `tls.rs`: X.509 cert generation, TLS verifier, peer pinning enforcement.
- `quic_transport.rs`: `ConnectionManager`, QUIC bind/connect/send, task ownership.
- `quic_transport_accept.rs`: inbound accept path (child module of `quic_transport.rs`): non-blocking accept loop, bounded pre-authentication handshake tasks, gated admission, connection-task ownership.
- `quic_transport_dial.rs`: outbound dial path (child module of `quic_transport.rs`): slot lookup, deadline-bounded + cancellation-aware resolution and dial-lock waits, configured-before-observed dial ordering with a per-endpoint fair-share budget, gated admission.
- `quic_transport_epochs.rs`: per-peer enrollment epochs and the synchronous (pin-snapshot + epoch) admission gate (child module of `quic_transport.rs`).
- `connection.rs`: Inbound/outbound stream lifecycle, message framing.
- `connection_streams.rs`: inbound uni/bidi stream handlers (child module of `connection.rs`).
- `connection_registry.rs`: generation-checked one-slot-per-peer connection ownership, cross-dial convergence (direction tie-break applies only inside the post-installation cross-dial window), and enrollment-epoch admission gating. The admission gate is a synchronous closure (pin snapshot + epoch); never reintroduce an await under the registry lock.
- `reconnect.rs`: versioned reconnect attempts and bounded exponential backoff; cancelled attempts must be released via `abandoned`, never left `in_flight`.
- `mod.rs`: Module exports, shared constants (`REQUEST_TIMEOUT`).

## Guardrails

- Never weaken TLS pinning — unknown peers must be rejected during handshake.
- Maintain one-message-per-stream semantics per `spec/WIRE_FORMAT.md` §4.1.
- Framing and size limits must match `spec/WIRE_FORMAT.md` §5.
- SNI must use full typed agent ID (`ed25519.<hex>`).
- Only `PeerDirectory` dial targets and immutable pinning snapshots may feed connection attempts.
- A stale connection or reconnect result must not replace or clear newer state.
- Every await in the send path recomputes remaining budget from the caller's absolute deadline; never hand a phase a fresh full budget.
- Every send-path await is either deadline-bounded or selects on a cancellation token; DNS resolution abandons its `spawn_blocking` worker on cancel/timeout (the worker's result is dropped, never consumed).
- Handshake attempts capture THEIR OWN PEER's enrollment epoch before they start (inbound handshakes snapshot all peers pre-handshake); admission re-checks that peer's epoch against revocations, so revoking one peer never rejects another's in-flight handshake. Epoch entries are never pruned (pruning would allow ABA reuse against restored trust).
- Admission consumes the published pinning snapshot as its enrollment oracle (the same snapshot TLS verification reads) — never live directory state under the registry lock.
- Revocations MUST go through `ConnectionManager::revoke_peer`: the directory commit and `close_peer` teardown are an inseparable, caller-detached pair (`PeerDirectory::remove_peer` is crate-private). The pair is spawned through the manager's task tracker, deliberately does NOT observe shutdown cancellation (teardown must follow commit), and is joined by `close_all`. A revocation racing shutdown may commit durably after the shutdown wait — safe, because the endpoint is already closed and there is no live connection to strand. Revocation completes even if the caller is cancelled; a bare directory removal could otherwise leave a just-admitted connection live against revoked trust.
- Enrollment-epoch lock poisoning fails closed at the admission gate; never recover a poisoned epoch lock and never panic on it.
- Every accepted connection and stream task must be owned and joined on shutdown.
- The accept loop must never await a handshake inline: every `Incoming` runs on its own tracked task, bounded by the pre-auth `handshake_semaphore`, so one half-open handshake cannot delay later inbound connections. The authoritative connection limit stays on `connection_semaphore` (checked only after mTLS completes).
- Outbound dials order configured locators before observed candidates and cap each endpoint to a fair share of the remaining exchange budget, so an unvalidated blackholed endpoint cannot starve the rest.
- A send retry only runs while the whole-exchange deadline still has time left; otherwise the informative first error is preserved.
- Dial-path deadline failures keep their timeout classification end to end: the dial lock, locator resolution, and QUIC handshake return `SendError::pre_send_timeout` (timed out, non-ambiguous — no payload was written), never a rewrapped `pre_send`. The daemon therefore reports `timeout` for every budget-exhausting send, never `peer_unreachable` (`spec/IPC.md` §5, Q-011).
- Inbound broadcast gaps are made observable through `subscribe_inbound_counted` + `inbound_gap_count`; the raw `subscribe_inbound` API stays source-compatible.

## Test targets

- Unit: `tls_tests.rs`, `quic_transport_tests.rs`, `connection_tests.rs`, `quic_transport_tests/accept_loop.rs` (accept loop + real handshake/revocation interleaving), `quic_transport_tests/dial_bounds.rs` (configured-first + per-endpoint budget + retry budget)
- Integration: `axon/tests/integration.rs`, `axon/tests/adversarial.rs`
