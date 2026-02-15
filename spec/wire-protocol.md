# AXON Wire Protocol Specification (v1)

This document defines the low-level wire format for AXON to ensure compatibility between different implementations.

## 1. Transport Layer: QUIC

AXON uses QUIC as its underlying transport.

- **QUIC Version**: 1 (RFC 9000).
- **ALPN**: The Application-Layer Protocol Negotiation string MUST be `axon-1`.
- **Default Port**: 7100/UDP.

## 2. Security: TLS 1.3

QUIC requires TLS 1.3. AXON uses self-signed certificates for peer authentication.

- **Handshake**: standard QUIC/TLS 1.3 handshake.
- **Certificate Requirements**:
    - MUST be an X.509 certificate.
    - MUST use an **Ed25519** public key.
    - The Subject Alternative Name (SAN) SHOULD contain the `agent_id` (32-char hex string) as a DNS name.
- **Verification**:
    - Implementations MUST NOT use standard CA-based validation.
    - Instead, they MUST extract the Ed25519 public key from the peer's certificate during the handshake.
    - This public key MUST match the public key associated with the `agent_id` provided during discovery (mDNS TXT records or static config).
    - If the keys do not match, the connection MUST be closed immediately with a `certificate_unknown` error.

## 3. Application Framing

Every message sent over a QUIC stream (unidirectional or bidirectional) MUST follow this framing:

| Offset | Length | Type | Description |
|--------|--------|------|-------------|
| 0      | 4      | u32  | Message length (N) in bytes, **Big-Endian**. |
| 4      | N      | byte | UTF-8 encoded JSON message. |

- **Max Message Size**: 65,536 bytes (64 KB). Implementations SHOULD close the stream if the length prefix exceeds this.
- **Encoding**: UTF-8 encoded JSON.

## 4. Stream Management

### Unidirectional Streams
Used for "fire-and-forget" messages (`notify`, `result`, `unsolicited error`).
- Each stream contains exactly **one** framed message.
- The sender MUST close the stream (FIN) after writing the message.

### Bidirectional Streams
Used for request/response patterns (`hello`, `ping`, `query`, `delegate`, `cancel`, `discover`).
- The initiator opens the stream and writes **one** framed request.
- The responder writes **one** framed response on the same stream.
- The responder MUST close the stream (FIN) after writing the response.
- The initiator MUST close its side of the stream (FIN) after reading the response.

## 5. Handshake: `hello`

- Every connection MUST begin with a `hello` exchange on the first available bidirectional stream.
- No other messages may be sent until the `hello` response is received and validated.
- The `v` (version) field in the envelope MUST be `1`.

## 6. Binary Representation of Identity

- **Public Key**: 32-byte Ed25519 raw bytes.
- **Agent ID**: First 16 bytes of SHA-256(raw public key), encoded as 32 hex characters (lowercase).
- **Base64**: All base64 strings in discovery (mDNS/TXT) or config MUST use the **Standard** alphabet with padding (RFC 4648).
