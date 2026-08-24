# AGENTS.md (transport)

This file applies to QUIC/TLS transport code in `axon/src/transport/`.

## Priorities

TLS security > protocol correctness > performance.

## File responsibilities

- `tls.rs`: X.509 cert generation, TLS verifier, peer pinning enforcement.
- `quic_transport.rs`: `ConnectionManager`, QUIC bind/connect/send, task ownership.
- `connection.rs`: Inbound/outbound stream lifecycle, message framing.
- `connection_registry.rs`: generation-checked one-slot-per-peer connection ownership and cross-dial convergence.
- `reconnect.rs`: versioned reconnect attempts and bounded exponential backoff.
- `mod.rs`: Module exports, shared constants (`REQUEST_TIMEOUT`).

## Guardrails

- Never weaken TLS pinning — unknown peers must be rejected during handshake.
- Maintain one-message-per-stream semantics per `spec/WIRE_FORMAT.md` §4.1.
- Framing and size limits must match `spec/WIRE_FORMAT.md` §5.
- SNI must use full typed agent ID (`ed25519.<hex>`).
- Only `PeerDirectory` dial targets and immutable pinning snapshots may feed connection attempts.
- A stale connection or reconnect result must not replace or clear newer state.
- Every accepted connection and stream task must be owned and joined on shutdown.

## Test targets

- Unit: `tls_tests.rs`, `quic_transport_tests.rs`, `connection_tests.rs`
- Integration: `axon/tests/integration.rs`, `axon/tests/adversarial.rs`
