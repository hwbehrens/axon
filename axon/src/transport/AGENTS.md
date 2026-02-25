# AGENTS.md (transport)

This file applies to QUIC/TLS transport code in `axon/src/transport/`.

## Priorities

TLS security > protocol correctness > performance.

## File responsibilities

- `tls.rs`: X.509 cert generation, TLS verifier, peer pinning enforcement.
- `quic_transport.rs`: QUIC bind, connect, send, endpoint management.
- `connection.rs`: Inbound/outbound stream lifecycle, message framing.
- `mod.rs`: Module exports, shared constants (`REQUEST_TIMEOUT`).

## Guardrails

- Never weaken TLS pinning — unknown peers must be rejected during handshake.
- Maintain one-message-per-stream semantics per `spec/WIRE_FORMAT.md` §4.1.
- Framing and size limits must match `spec/WIRE_FORMAT.md` §5.
- SNI must use full typed agent ID (`ed25519.<hex>`).

## Test targets

- Unit: `tls_tests.rs`, `quic_transport_tests.rs`, `connection_tests.rs`
- Integration: `axon/tests/integration.rs`, `axon/tests/adversarial.rs`
