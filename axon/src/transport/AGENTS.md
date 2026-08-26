# AGENTS.md (transport)

This file applies to QUIC/TLS transport code in `axon/src/transport/`.

## Priorities

TLS security > protocol correctness > performance.

## File responsibilities

- `tls.rs`: X.509 cert generation, TLS verifier, peer pinning enforcement.
- `quic_transport.rs`: `ConnectionManager`, QUIC bind/connect/send, task ownership.
- `quic_transport_dial.rs`: outbound dial path (child module of `quic_transport.rs`): slot lookup, deadline-bounded + cancellation-aware resolution and dial-lock waits, gated admission.
- `quic_transport_epochs.rs`: per-peer enrollment epochs and the fail-closed, budget-bounded admission gate (child module of `quic_transport.rs`).
- `connection.rs`: Inbound/outbound stream lifecycle, message framing.
- `connection_streams.rs`: inbound uni/bidi stream handlers (child module of `connection.rs`).
- `connection_registry.rs`: generation-checked one-slot-per-peer connection ownership, cross-dial convergence (direction tie-break applies only inside the post-installation cross-dial window), and enrollment-epoch admission gating.
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
- Every accepted connection and stream task must be owned and joined on shutdown.

## Test targets

- Unit: `tls_tests.rs`, `quic_transport_tests.rs`, `connection_tests.rs`
- Integration: `axon/tests/integration.rs`, `axon/tests/adversarial.rs`
