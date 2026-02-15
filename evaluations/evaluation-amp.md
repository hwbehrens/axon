# AXON Evaluation — `agent/amp` Branch

_Oracle assessment — Feb 14, 2026 (from-first-principles evaluation against spec and rubric)_

## Test Results

| Suite | Count | Status |
|---|---:|---|
| Unit tests | 136 | ✅ All pass |
| Integration tests | 22 | ✅ All pass |
| Spec compliance tests | 46 | ✅ All pass |
| E2E tests | 3 | ✅ All pass |
| Compilation | — | ✅ Clean, no warnings |

**Total: 207 tests passing, 0 ignored.**

---

## Summary Scores

| Category | Weight | Score |
|---|---|---:|
| 1. Spec Compliance | ×2 | **18/20** |
| 2. Correctness | ×1 | **9/10** |
| 3. Code Quality | ×1 | **8/10** |
| 4. Test Coverage | ×1 | **9/10** |
| 5. Security | ×1 | **9/10** |
| 6. Concurrency & Async | ×1 | **8/10** |
| 7. Error Handling | ×1 | **8/10** |
| 8. Completeness | ×1 | **8/10** |
| 9. Production Readiness | ×1 | **8/10** |
| **Total** | | **85/100** |

---

## 1. Spec Compliance (18/20)

### Strengths

- **Agent ID format matches spec (`ed25519.` + 32 hex chars)**
  - Derivation is `SHA-256(pubkey)`, first 16 bytes, hex-encoded, prefixed with `ed25519.`: `identity.rs:137-141`.
  - TLS derives the same way: `tls.rs:208-212`.
  - Envelope validation enforces `ed25519.` prefix + 32 hex chars: `envelope.rs:101-106`.

- **Wire format is FIN-delimited JSON (no length prefix, one message per QUIC stream)**
  - Unidirectional send writes raw JSON bytes and finishes the stream: `framing.rs:8-26` (`write_all` at `framing.rs:60-63`, `finish` at `framing.rs:22-25`).
  - Request/response uses a bidirectional stream, finishes send side, reads to FIN: `framing.rs:29-53` (`finish` at `framing.rs:43-46`, `read_to_end` at `framing.rs:68-72`).
  - `wire.rs` encodes/decodes via `serde_json` without length prefix: `wire.rs:9-23`.

- **All 13 message kinds present with typed payload structs**
  - `MessageKind` enum covers all required/optional kinds plus `#[serde(other)] Unknown` for forward compatibility: `kind.rs:5-23`.
  - Typed payload structs for all 13 kinds with correct serde derives: `payloads.rs:63-176`.

- **Stream mapping and protocol-violation handling match spec §10**
  - Unknown kind on unidirectional: silently dropped (`connection.rs:78-80`).
  - Unknown kind on bidirectional: returns `error(unknown_kind)` (`connection.rs:170-185`).
  - Request kind on unidirectional: dropped (`connection.rs:80-81`).
  - Fire-and-forget on bidirectional: forwarded, send side closed without responding (`connection.rs:186-188`).
  - Pre-hello unidirectional: dropped (`connection.rs:76-78`).
  - Pre-hello bidirectional: `error(not_authorized)` without closing connection (`connection.rs:154-169`).
  - Version mismatch: returns `error(incompatible_version)` (`handshake.rs:14-29`), then connection is closed via `break` in `connection.rs:151-152`.
  - Agent ID mismatch on unidirectional: checked (`connection.rs:76`).
  - Duplicate message ID (replay): bounded UUID cache with 300s TTL and 100K cap (`daemon/replay_cache.rs:84-118`, `daemon/mod.rs:129-132`).
  - Oversized messages: enforced at read via `read_to_end(MAX_MESSAGE_SIZE_USIZE)` (`framing.rs:68-72`) and at write (`framing.rs:56-58`).
  - Malformed JSON: dropped on both uni and bidi (`connection.rs:88-90`, `connection.rs:109-112`).

- **Connection lifecycle matches spec**
  - Lower agent_id initiates: `daemon/command_handler.rs:86-106`, `daemon/reconnect.rs:47-51`.
  - Keepalive 15s / idle timeout 60s: `transport/mod.rs:11-14`, applied in `tls.rs:46-51`.
  - Handshake deadline 5s for unauthenticated connections: `connection.rs:13`, `connection.rs:57-69`.
  - Reconnect with exponential backoff capped at 30s: `daemon/reconnect.rs:15-28`.
  - QUIC stream caps: 8 bidi, 16 uni: `tls.rs:47-48`.
  - Read timeout 10s on inbound streams: `connection.rs:14`, used at `connection.rs:73-98` and `connection.rs:106-122`.

- **Discovery trait with correct implementations**
  - `Discovery` trait + `StaticDiscovery` + `MdnsDiscovery`: `discovery.rs:27-63`, `discovery.rs:68-161`.
  - Service type `_axon._udp.local` matches spec §2: `discovery.rs:13`.
  - Discovered peer cap (1024): `peer_table.rs:83`.
  - Stale peer removal clears expected pubkeys: `daemon/mod.rs:233-239`.

- **IPC matches spec (Unix socket, line-delimited JSON, bounded queues)**
  - Stale socket cleanup on startup: `server.rs:36-43`.
  - Socket permissions 0600: `server.rs:52-61`.
  - Per-client bounded outbound queue (1024): `server.rs:16-18`, `server.rs:148-150`.
  - Max IPC line length (256KB): `server.rs:17-18`, `server.rs:189-202`.
  - Commands: `send`, `peers`, `status`: `protocol.rs:11-24`.
  - Max connection semaphore: `quic_transport.rs:239-248`.

- **CLI commands match spec and beyond**
  - All spec-required: `daemon`, `send`, `delegate`, `notify`, `peers`, `status`, `identity`: `main.rs:24-65`.
  - Additional: `ping`, `discover`, `cancel`, `examples`: `main.rs:45-65`.

- **Daemon lifecycle implements startup sequence, runtime routing, and graceful shutdown per spec §8**
  - Startup: `daemon/mod.rs:47-113`.
  - Runtime event loop: `daemon/mod.rs:204-259`.
  - Shutdown: close connections, save state, remove socket: `daemon/mod.rs:261-280`.

- **Config file support for static peers and port**: `config.rs:56-98`.
- **Max message size 64KB**: `wire.rs:7`.

### Deductions (-2)

- **Bidirectional fire-and-forget messages forwarded without envelope validation** (-1)
  - `connection.rs:186-188` forwards `request` and finishes without calling `request.validate()`.
  - Contrast: unidirectional path validates (`connection.rs:82-84`); bidirectional request-response path validates (`connection.rs:189-192`).

- **Transport timing knobs (keepalive, idle timeout) are constants, not configurable** (-1)
  - `KEEPALIVE_INTERVAL` / `IDLE_TIMEOUT` are compile-time constants: `transport/mod.rs:11-14`, used in `tls.rs:46-51`.
  - No configuration surface in `config.rs:56-91`. Spec says these are RECOMMENDED values and implementations SHOULD make them configurable.

---

## 2. Correctness (9/10)

### Strengths

- All 207 tests pass with clean build, zero warnings.
- Correct QUIC stream usage: request path opens bidi, sends, finishes, reads with timeout (`framing.rs:38-53`); unidirectional path opens uni, writes, finishes (`framing.rs:17-26`).
- Hello authentication correctly ties `hello.from` to certificate pubkey: `handshake.rs:100-106`.
- Connection loop has clear shutdown paths: cancellation exits (`connection.rs:60-64`), handshake deadline closes (`connection.rs:65-69`), connection tracking cleanup removes from map (`connection.rs:207-209`).
- Daemon startup/shutdown path is structured and consistent (`daemon/mod.rs:47-113`, `daemon/mod.rs:261-280`).
- Integration tests exercise full daemon lifecycle including reconnect (`tests/daemon_lifecycle.rs:105-246`).

### Deductions (-1)

- Validation gap on bidi fire-and-forget: invalid envelopes can reach IPC subscribers (`connection.rs:186-188`).

---

## 3. Code Quality (8/10)

### Strengths

- Clear module boundaries matching spec architecture (identity/message/config/discovery/transport/ipc/daemon).
- Forward-compatible envelope payload handling: `payload` stored as `Box<RawValue>` to avoid unnecessary parse/serialize and preserve unknown fields (`envelope.rs:21-25`).
- Consistent `anyhow::Context` usage for error chains throughout.
- Thoughtful handling of rustls verifier needing sync locks with inline comments explaining why (`transport/tls.rs:120-125`).
- Replay cache is well-implemented: bounded + TTL + ordered eviction (`daemon/replay_cache.rs:84-118`).
- Idiomatic `tokio::select!` loops with cancellation tokens (`connection.rs:59-204`, `daemon/mod.rs:205-259`).

### Deductions (-2)

- Some "stringly-typed" payload construction despite having typed payload structs — error/hello responses built via `json!()` rather than using `ErrorPayload`/`HelloPayload` structs: `handshake.rs:15-28`, `connection.rs:127-136`, `connection.rs:155-164`, `connection.rs:171-180` (-1).
- `AgentId` is a type alias (`String`) rather than a newtype, missing compile-time distinction: `envelope.rs:12` (-1).

---

## 4. Test Coverage (9/10)

### Strengths

- 207 tests across 4 suites: unit (136), integration (22), spec compliance (46), and E2E (3).
- Full daemon lifecycle + reconnect tests (`tests/daemon_lifecycle.rs:105-246`).
- Broad integration coverage across subsystems (`tests/integration.rs`).
- Serde round-trips, wire format, typed payloads, and spec-example deserialization covered (`tests/spec_compliance.rs`).
- Spec compliance tests verify agent ID format with `ed25519.` prefix and 40-char length (`tests/spec_compliance.rs:671-681`).
- Property-based tests via proptest in 7 modules; 6 fuzz targets (per AGENTS.md).
- IPC tests cover multi-client broadcast, disconnect isolation, invalid commands, oversized lines (`tests/integration.rs:548-787`).

### Deductions (-1)

- Limited explicit tests for all 8 protocol violation types from updated spec §10 — most violations are covered by transport-level behavior but not exercised by dedicated tests.

---

## 5. Security (9/10)

### Strengths

- Filesystem permissions: private key chmod 600 (`identity.rs:50-56`), root dir chmod 700 (`config.rs:40-52`), socket chmod 600 (`ipc/server.rs:54-61`).
- mTLS pinning: reject unknown peers unless discovery/config has pubkey (`transport/tls.rs:120-136`, `transport/tls.rs:170-175`).
- Early data (0-RTT) disabled at both client and server (`transport/tls.rs:40-41`, `transport/tls.rs:64-65`).
- Replay cache bounded + TTL (`daemon/replay_cache.rs`).
- IPC bounded per-client outbound queue with drop on full (`ipc/server.rs:16-18`, `ipc/server.rs:79-82`).
- Connection-level DoS mitigation via max connection semaphore (`quic_transport.rs:239-248`) that tracks ALL inbound connections (including unauthenticated ones), combined with 5s handshake deadline.
- Stale peer removal also removes trust (expected_pubkeys): `daemon/mod.rs:237`.

### Deductions (-1)

- Blocking filesystem IO occurs inside async functions (minor availability risk under load): replay cache `save()` uses `std::fs::write` (`replay_cache.rs:79-81`), known peers persistence uses `std::fs::write` (`config.rs:127-130`) called from async daemon loop (`daemon/mod.rs:227-229`, `daemon/mod.rs:254-256`).

---

## 6. Concurrency & Async Design (8/10)

### Strengths

- `CancellationToken` used end-to-end for structured shutdown (`daemon/mod.rs:74-76`, `transport/quic_transport.rs:30-31`).
- Max connection semaphore for inbound connections with permit dropped after connection loop exits (`quic_transport.rs:239-268`).
- Per-peer connect mutex prevents concurrent dial storms (`quic_transport.rs:101-115`).
- Read timeouts on streams (`transport/connection.rs:14`).
- IPC bounded per-client queue with drop-on-full semantics (`ipc/server.rs:16-18`).
- Replay cache uses `tokio::sync::Mutex` (`daemon/replay_cache.rs`).

### Deductions (-2)

- Blocking file IO inside async context: `replay_cache.rs:79-81`, `config.rs:127-130`, used in async daemon loop at `daemon/mod.rs:254-256` (-1).
- Mixed `std::sync` locks and async code increases risk of accidental blocking — `expected_pubkeys` uses `StdRwLock` (`quic_transport.rs:26-28`), accessed in async setters (`quic_transport.rs:77-87`). Justified by rustls sync callbacks (`tls.rs:120-124`, `tls.rs:159-163`) but easy to misuse (-1).

---

## 7. Error Handling (8/10)

### Strengths

- Instructive IPC send errors with remediation hints (`daemon/command_handler.rs:46-52`).
- Malformed messages dropped without crashing (`connection.rs:88-90`, `connection.rs:109-112`).
- Good contextual errors for config/IO (`config.rs:69-77`, `ipc/server.rs:36-43`).
- TLS rejection errors are instructive: suggest adding peer to config.toml or ensuring mDNS discovery (`tls.rs:131-136`, `tls.rs:170-175`).
- Hello version mismatch explains local vs peer supported versions (`quic_transport.rs:184-192`).
- Hello rejection error includes actionable guidance (`quic_transport.rs:194-206`).
- Clock validation at startup with instructive error (`daemon/mod.rs:65-72`).

### Deductions (-2)

- Some protocol violations are logged as warnings rather than truly "silently" dropped — operationally useful but may be noisy under adversarial conditions: `connection.rs:78-80`, `connection.rs:80-81` (-1).
- Bidi invalid request is silently dropped without any response (`connection.rs:189-191`), which is spec-correct but can make debugging harder since the peer sees only a timeout (-1).

---

## 8. Completeness (8/10)

### Strengths

- All major subsystems present and wired: daemon orchestration, QUIC transport with mTLS, discovery (mDNS + static), peer table (caps + stale cleanup), IPC server + protocol, replay cache with persistence, CLI commands, reconnect with backoff.
- Beyond-spec additions: replay cache persistence across restarts, `axon examples` command, configurable resource limits (`max_ipc_clients`, `max_connections`), clock validation, `axon cancel`/`axon ping`/`axon discover` CLI commands.
- E2E tests verify full daemon lifecycle including reconnection after peer restart.

### Deductions (-2)

- Replay protection is enforced at daemon layer only (`daemon/mod.rs:129-132`), not at transport layer — `connection.rs` forwards envelopes to subscribers before any replay check (`connection.rs:85-86`, `connection.rs:187-188`, `connection.rs:192`). If `QuicTransport` is used as a library without the daemon, replay protection is absent (-1).
- `auto_response()` provides canned responses for all bidi request kinds (`handshake.rs:11-93`), meaning the transport layer responds to queries/delegates instead of routing to IPC clients for real application-level responses. This limits the system to a message router with stub responses rather than full application integration (-1).

---

## 9. Production Readiness (8/10)

### Strengths

- Logging via `tracing` + `EnvFilter` (`main.rs:350-353`).
- Config file support with defaults (`config.rs:56-91`).
- Known peers persistence on events + periodic (`daemon/mod.rs:227-229`, `daemon/mod.rs:254-256`).
- Graceful shutdown: close connections, save state, remove socket (`daemon/mod.rs:261-280`).
- Reconnect with exponential backoff capped at 30s (`daemon/reconnect.rs:15-28`).
- SIGTERM/SIGINT both handled for systemd/launchd compatibility (`main.rs:80-89`).
- Replay cache persisted across restarts (`daemon/replay_cache.rs:36-59`, `daemon/mod.rs:275-277`).

### Deductions (-2)

- Some transport constants (keepalive, idle timeout, reconnect backoff parameters) not configurable via config.toml — reduces operational flexibility (-1).
- Blocking persistence in runtime loop: `std::fs::write` in async context for known peers and replay cache saves (-1).

---

## Highest-Leverage Improvements

1. **Validate envelopes for bidirectional fire-and-forget messages before forwarding.** (~S effort)
   - Add `request.validate()` gating to the `!expects_response()` branch in `connection.rs:186-188` to match uni/bidi validation behavior.

2. **Make transport timing knobs configurable (keepalive interval, idle timeout, reconnect backoff).** (~M effort)
   - Extend `Config` (`config.rs:56-66`) to include optional fields; thread them into `tls.rs:46-51` and `reconnect.rs:15-28`.

3. **Move runtime persistence off the async reactor.** (~M effort)
   - Replace `std::fs::write` in async contexts with `tokio::fs::write` or `tokio::task::spawn_blocking`: `daemon/mod.rs:254-256`, `replay_cache.rs:79-81`, `config.rs:127-130`.

4. **Use typed payload structs instead of `json!()` for protocol responses.** (~S effort)
   - Replace `json!()` calls in `handshake.rs` and `connection.rs` with `ErrorPayload`, `HelloPayload`, etc. for compile-time correctness.
