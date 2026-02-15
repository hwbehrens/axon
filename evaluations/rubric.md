# AXON Implementation Scoring Rubric

Total: 100 points across 9 categories.

## 1. Spec Compliance (×2 weight, max 20)
Does the implementation faithfully follow the spec? Check:
- Envelope format (v, id, from, to, ts, kind, ref, payload)
- All message kinds and payload schemas (hello, ping/pong, query/response, delegate/ack/result, notify, cancel, discover/capabilities, error)
- Stream mapping (uni vs bidi per spec table)
- Wire format (u32 big-endian length prefix + JSON)
- Identity: Ed25519 keypair, agent ID = first 16 bytes of SHA-256(pubkey) hex-encoded
- Self-signed X.509 cert from Ed25519 keypair via rcgen
- Discovery trait with PeerEvent enum, MdnsDiscovery and StaticDiscovery
- IPC: Unix socket at ~/.axon/axon.sock, line-delimited JSON, commands (send/peers/status), responses
- Connection lifecycle: lower agent_id initiates, cert validation, keepalive 15s/idle 60s, reconnect with exp backoff
- CLI commands: daemon, send, delegate, notify, peers, status, identity
- Daemon lifecycle: startup sequence, runtime routing, graceful shutdown
- Config: ~/.axon/config.toml with static peers, port config
- Max message size 64KB

## 2. Correctness (max 10)
Does the code actually work? Would it compile and run correctly? Check for logic errors, race conditions, missing error handling at boundaries, broken control flow.

## 3. Code Quality (max 10)
Idiomatic Rust? Clean module boundaries? Proper use of types, traits, error handling (anyhow/thiserror)? No unnecessary complexity?

## 4. Test Coverage (max 10)
Are there unit tests covering public functions, edge cases, serde round-trips, crypto ops, peer table ops? Are there integration tests? Do tests actually verify meaningful invariants?

## 5. Security (max 10)
Private key permissions (chmod 600)? Cert validation against discovery pubkey? No secret logging? Stale socket cleanup? Proper TLS configuration?

## 6. Concurrency & Async Design (max 10)
Proper use of tokio? Safe shared state (Arc/Mutex/RwLock)? No blocking in async context? Clean shutdown with cancellation?

## 7. Error Handling (max 10)
Graceful degradation? Instructive error messages per spec? Proper Result propagation? No panics in non-test code?

## 8. Completeness (max 10)
What percentage of the spec is actually implemented vs stubbed? Are all modules functional or are some empty shells?

## 9. Production Readiness (max 10)
Logging/tracing? Config file support? Reconnection logic? Graceful shutdown? Could this actually run as a daemon?

## Summary Table

| Category | Weight | agent/codex | agent/amp | agent/claude | agent/gemini |
|---|---|---|---|---|---|
| Spec Compliance | ×2 | 13/20 | 18/20 | 12/20 | 4/20 |
| Correctness | ×1 | 7/10 | 8/10 | 6/10 | 2/10 |
| Code Quality | ×1 | 8/10 | 8/10 | 7/10 | 5/10 |
| Test Coverage | ×1 | 7/10 | 9/10 | 7/10 | 2/10 |
| Security | ×1 | 8/10 | 8/10 | 4/10 | 2/10 |
| Concurrency & Async | ×1 | 6/10 | 7/10 | 6/10 | 4/10 |
| Error Handling | ×1 | 7/10 | 7/10 | 7/10 | 4/10 |
| Completeness | ×1 | 7/10 | 9/10 | 6/10 | 2/10 |
| Production Readiness | ×1 | 6/10 | 7/10 | 6/10 | 3/10 |
| **Total** | | **69/100** | **81/100** | **61/100** | **28/100** |
