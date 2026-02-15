# AXON Implementation Evaluation (agent/amp) — Rubric Scoring

This document evaluates the AXON implementation against [`RUBRIC.md`](../RUBRIC.md) (mature codebase rubric). Scores start at category max and deduct for concrete issues/risks observed, with code references.

---

## 1) Security & Hardening — **15 / 18** (−3)

AXON's core security posture is strong: mutual TLS with peer pinning, identity binding, hello gating, replay protection, and multiple DoS controls are implemented in a way that is consistent and auditable.

### What's working well (max-point items satisfied)

- **mTLS peer pinning + identity binding**
  - Outbound verification pins the server cert's Ed25519 pubkey to the expected peer table and *also* binds SNI → derived agent id:
    - `tls.rs:89-139` (`PeerCertVerifier::verify_server_cert`) derives agent id from cert pubkey (`tls.rs:108-118`) and compares to SNI-derived `expected_agent_id` (`tls.rs:99-107`), then checks expected pubkey record (`tls.rs:120-136`).
  - Inbound verification rejects unknown clients unless in expected table:
    - `tls.rs:142-178` (`PeerClientCertVerifier::verify_client_cert`) derives agent id from cert pubkey (`tls.rs:153-158`) and checks `expected_pubkeys` (`tls.rs:160-175`).
  - Transport uses the peer's `agent_id` as the QUIC SNI:
    - `quic_transport.rs:154-161` (`endpoint.connect(peer.addr, &peer.agent_id)`).

- **Handshake and authorization gates**
  - "Hello must be first" is enforced for both uni and bidi:
    - Uni: dropped pre-hello (`connection.rs:79-86`).
    - Bidi: pre-hello non-hello gets `error(not_authorized)` (`connection.rs:164-182`).

- **Replay protection is bounded and applied on inbound paths**
  - Uni replay drop (`connection.rs:88-93`).
  - Bidi replay drop for fire-and-forget (`connection.rs:204-210`) and request kinds (`connection.rs:232-237`).
  - Replay cache is TTL + max entries (`replay_cache.rs:11-15`, `replay_cache.rs:85-119`), with explicit eviction (`replay_cache.rs:106-117`).

- **Resource/DoS controls**
  - Max message size enforced at framing send and read limits:
    - `framing.rs:13-15`, `framing.rs:34-36`, `framing.rs:67-72` (read bounded by `MAX_MESSAGE_SIZE_USIZE`).
  - Connection caps:
    - Inbound accept loop uses a semaphore + hard reject on limit (`quic_transport.rs:279-288`).
  - IPC controls:
    - Bounded per-client queue (`server.rs:16-18`, `server.rs:148-150`), max clients (`server.rs:136-145`), and line-length limit (`server.rs:190-202`).

- **Secrets / permissions**
  - Private key file permission is explicitly `0600` (`identity.rs:50-56`) and config root dir `0700` (`config.rs:41-54`); socket `0600` (`server.rs:52-61`).

### Deductions (issues/risks)

1) **Hello envelope is not run through general `Envelope::validate()` before being accepted/broadcast** (−1)
   - In `connection.rs`, when a bidi `Hello` arrives, identity is checked (`connection.rs:134-147`) and on success the connection is authenticated and the hello request is broadcast (`connection.rs:154-161`) **without** `request.validate()` being called for the hello itself.
   - `validate_hello_identity()` only checks `hello.from` matches derived agent id and an *optional* expected key consistency check (`handshake.rs:118-140`), but does not validate:
     - `hello.to` format / correctness,
     - `hello.ts != 0`,
     - general envelope invariants in `Envelope::validate()` (`envelope.rs:170-180`).
   - Practical impact: a peer can send a structurally "authenticated" hello with malformed `to` or `ts=0` that will enter the inbound broadcast stream and IPC.

2) **IPC socket cleanup removes any pre-existing filesystem entry at `socket_path` without validating it is a socket** (−1)
   - `IpcServer::bind()` unconditionally `remove_file()` if `socket_path.exists()` (`server.rs:36-43`).
   - In a correctly permissioned `~/.axon` root (`config.rs:47-52`) this is likely safe in practice; however if `axon_root` were placed in a less trusted directory (or permissions drift), this becomes a footgun (e.g., symlink/hardlink attacks) because it will delete whatever file is there.

3) **`Envelope::payload_value()` silently converts JSON parse errors into `Value::Null`** (−1)
   - `envelope.rs:124-127` uses `unwrap_or(Value::Null)`.
   - While convenient, this is a hardening concern because it can mask malformed payloads and lead to "default behavior" paths instead of explicit rejection/logging at the decision point (especially for request handling / auto-response decisions that inspect payload fields).

---

## 2) Test Quality & Coverage — **16 / 18** (−2)

The project has unusually comprehensive testing for a network daemon: unit tests, integration tests, spec compliance, adversarial tests, property-based testing, fuzz targets, and benches.

### What's working well

- **Spec compliance suite is explicit and broad**
  - `tests/spec_compliance.rs` asserts envelope schema shape, payload round-trips, unknown kind handling, and violation behaviors (e.g., `spec_compliance.rs:94-110`, `spec_compliance.rs:390-404`, `spec_compliance.rs:467-478`).
- **Adversarial tests cover concurrency, malformed inputs, corruption resilience**
  - IPC flood (`tests/adversarial.rs:37-120`), peer table contention (`tests/adversarial.rs:126-196`), malformed IPC handling, persistence corruption tests, and wire decode edge cases.
- **Integration tests cover cross-module flows**
  - QUIC transport handshake/query flows and hello-first invariant (`tests/integration.rs:321-363`), uni fire-and-forget delivery (`tests/integration.rs:404-456`).
- **Fuzz targets are first-class in the build tooling**
  - Makefile runs 6 fuzz targets (`Makefile:37-46`).

### Deductions

1) **Long-running daemon lifecycle tests are `#[ignore]` and therefore not exercised by default CI (`make verify` / `cargo test`)** (−1)
   - `tests/daemon_lifecycle.rs:7-8` indicates lifecycle e2e tests are ignored.
   - This is reasonable for local workflows, but per rubric it reduces regression detection for the most operationally critical behaviors (shutdown, reconnection, initiator rule).

2) **Some tests rely on real time sleeps rather than deterministic signaling** (−1)
   - Example patterns: `tokio::time::sleep(...)` used for readiness/ordering in the daemon lifecycle tests (`daemon_lifecycle.rs:82-83`, `daemon_lifecycle.rs:196-198`, etc.) and in some adversarial/integration flows (e.g., client count stabilization).
   - This is not necessarily flaky today, but it's a common source of eventual CI flakes under load.

---

## 3) Performance & Efficiency — **12 / 14** (−2)

Core hot paths are reasonably efficient (bounded reads, RawValue payload preservation, avoided double parses where possible), and concurrency is bounded. No major performance footguns are apparent.

### What's working well

- **No repeated JSON parsing on the wire payload by default**
  - Envelope stores payload as `RawValue` (`envelope.rs:101-102`) and round-trips preserve raw JSON (`spec_compliance.rs` round-trip assertions).
- **Async correctness in most operational paths**
  - Persistence writes use `tokio::fs` (`config.rs:138-151`, `replay_cache.rs:79-82`), reducing reactor blocking for periodic saves.
- **Bounded concurrency/backpressure**
  - Inbound connections are bounded via semaphore (`quic_transport.rs:279-312`).
  - IPC output queues are bounded and drop-on-overflow using `try_send` (`server.rs:76-83`, `server.rs:85-96`).

### Deductions

1) **Pretty-printed JSON for persistence adds unnecessary CPU/IO overhead** (−1)
   - `serde_json::to_vec_pretty` is used for replay cache saves (`replay_cache.rs:78-82`) and known peer saves (`config.rs:147-151`).
   - These aren't the hottest paths, but they *are periodic* (daemon saves known peers every 60s: `mod.rs:200-268`) and can become noticeable on slower storage / high peer counts.

2) **Potential avoidable cloning in inbound forwarding** (−1)
   - In daemon inbound forwarder, an `Arc<Envelope>` is converted into an owned `Envelope` via `Arc::try_unwrap(...).unwrap_or_else(|arc| (*arc).clone())` (`mod.rs:149-150`).
   - This is acceptable, but under high fanout it can lead to extra envelope clones; it's a small efficiency hit in a path that may run frequently.

---

## 4) Maintainability & Code Quality — **12 / 14** (−2)

The codebase is cleanly modular, consistent, and mostly uses typed structures rather than "stringly typed" JSON. Error handling is generally contextual and readable.

### What's working well

- **Clear module boundaries and consistent patterns**
  - Transport separation: TLS verifier (`tls.rs`), connection loop (`connection.rs`), framing (`framing.rs`), handshake helpers (`handshake.rs`), transport orchestration (`quic_transport.rs`).
- **Typed payload structs and message kinds**
  - `payloads.rs` provides structured payloads for hello/ping/query/etc. and typed enums (`payloads.rs:64-177`).
  - `MessageKind` includes a forward-compat `Unknown` variant via `#[serde(other)]` (`kind.rs:21-23`).
- **Invariant checks are centralized**
  - Envelope invariants (`envelope.rs:170-189`).
  - Connection hello gating and stream-type rules in one place (`connection.rs`).

### Deductions

1) **Agent identity types are not used consistently across boundaries** (−1)
   - `Envelope` uses `AgentId` newtype (`envelope.rs:12-23`), but much of transport and peer table uses raw `String` (`quic_transport.rs:34-46`, `peer_table.rs:31-39`).
   - This weakens type-driven correctness for "load-bearing invariants" like agent id format and comparison rules.

2) **`payload_value()` error swallowing reduces auditability** (−1)
   - As noted in Security: `envelope.rs:124-127` turning malformed payload JSON into `Null` can obscure root cause and produce surprising behavior in higher-level logic that inspects payload fields (e.g., `handshake.rs:143-150` checks `protocol_versions` array).

---

## 5) Operational Maturity — **8 / 10** (−2)

AXON behaves like a real daemon: structured shutdown, periodic persistence, reconnection with backoff, and basic counters/status via IPC.

### What's working well

- **Structured lifecycle and graceful shutdown**
  - Cancellation-driven main loop and background tasks (`mod.rs:214-270`).
  - Shutdown drains briefly, closes transport, persists state, cleans up socket (`mod.rs:272-292`).
- **Configurability of key knobs**
  - Connection limits, IPC client limits, keepalive/idle timeout, reconnect max backoff are configurable (`config.rs:57-110`) and applied (`mod.rs:104-114`, `mod.rs:256-262`).
- **Reconnect behavior with backoff**
  - Exponential backoff with cap (`reconnect.rs:23-28`, `reconnect.rs:89-100`) and initiator rule enforcement (`mod.rs:194-197`, `command_handler.rs:86-107`).

### Deductions

1) **Some warning-level logs could be spammy under adversarial conditions and lack peer identifiers** (−1)
   - Example: repeated `warn!("uni stream read timed out")` (`connection.rs:104-106`) and `warn!("bidi stream read timed out")` (`connection.rs:127-129`) without tagging peer/agent id.
   - This can become a log-volume DoS vector and reduces operational debugging value.

2) **Some important timeouts are hard-coded rather than configurable** (−1)
   - `HANDSHAKE_TIMEOUT` (5s) and `INBOUND_READ_TIMEOUT` (10s) are constants (`connection.rs:12-13`).
   - This is not inherently wrong, but in production environments these often need tuning (slow systems, high latency links, mobile networks, etc.).

---

## 6) Adversarial Robustness — **10 / 10** (−0)

The implementation demonstrates strong robustness practices:

- Dedicated adversarial suite (`tests/adversarial.rs`) covering concurrent IPC flood, malformed input, contention, disconnects, and file corruption resilience.
- Wire-level resilience assertions (decode errors, wrong schema types).
- Fuzz targets for core parsing surfaces are integrated into the Makefile (`Makefile:37-46`), and replay cache is bounded and TTL-scavenged (`replay_cache.rs:85-119`).
- Protocol violation handling is explicit: uni drops and bidi returns typed errors (`connection.rs:79-253`).

No concrete robustness regressions were identified from the provided code.

---

## 7) Contribution Hygiene — **9 / 10** (−1)

Project hygiene is strong and matches expectations for a mature Rust repository.

### What's working well

- **One-command local verification**
  - `make verify` runs fmt + clippy(warnings as errors) + full tests (`Makefile:79-81`).
- **Fuzz/coverage/mutation tooling present**
  - Fuzz targets runnable via Makefile (`Makefile:37-52`).
  - Coverage (`Makefile:54-64`).
  - Mutation testing via `cargo mutants` (`Makefile:65-72`).

### Deduction

1) **End-to-end daemon lifecycle coverage is not part of the default verify path** (−1)
   - `make verify` does not run `test-e2e` (`Makefile:79-81` vs `Makefile:15-17`), and the e2e tests are `#[ignore]` (`daemon_lifecycle.rs:7-8`).
   - This is an explicit trade-off; still, it weakens the "PR contains the whole change, CI-driven" posture for daemon lifecycle regressions.

---

## 8) Interop & Spec Drift Control — **4 / 6** (−2)

There is substantial investment in spec compliance testing, unknown-field tolerance, and wire invariants, but there are a couple of drift risks worth flagging.

### What's working well

- **Forward compatibility**
  - Unknown envelope fields ignored (`tests/spec_compliance.rs:94-110`), and `MessageKind::Unknown` supports future kinds (`kind.rs:21-23`, `spec_compliance.rs:390-404`).
- **Wire invariants tested**
  - Max size rejection via encode test (`spec_compliance.rs:467-478`).
  - Stream-type rules are encoded in transport (`connection.rs:81-85`, `connection.rs:182-200`) and classification tests exist (`spec_compliance.rs:406-444`).

### Deductions

1) **Spec compliance tests use agent_id strings without the `ed25519.` prefix in some helpers** (−1)
   - `tests/spec_compliance.rs:13-19` returns `"a1b2..."` / `"f6e5..."` without prefix, while the actual daemon/identity derives IDs in `"ed25519.<32hex>"` form (`tls.rs:208-212`, `identity.rs:137-141`) and `Envelope::validate()` enforces this format (`envelope.rs:174-188`).
   - This mismatch increases the risk of spec drift: tests may pass while not accurately representing real-world IDs and validation behavior.

2) **`wire::decode()` does not enforce the 64KiB max size invariant by itself** (−1)
   - `wire.rs:20-23` decodes any slice; size checks only exist in `encode()` (`wire.rs:9-17`) and framing read limits (`framing.rs:67-72`).
   - This is likely fine given how the code uses it today, but it's an interop/spec-control risk because `decode()` is a public entrypoint and may be used elsewhere without a size guard.

---

## Summary Table

| Category | Max | agent/codex | agent/amp | agent/claude | agent/gemini |
|---|---:|---:|---:|---:|---:|
| 1. Security & Hardening | 18 |  | **15** |  |  |
| 2. Test Quality & Coverage | 18 |  | **16** |  |  |
| 3. Performance & Efficiency | 14 |  | **12** |  |  |
| 4. Maintainability & Code Quality | 14 |  | **12** |  |  |
| 5. Operational Maturity | 10 |  | **8** |  |  |
| 6. Adversarial Robustness | 10 |  | **10** |  |  |
| 7. Contribution Hygiene | 10 |  | **9** |  |  |
| 8. Interop & Spec Drift Control | 6 |  | **4** |  |  |
| **Total** | **100** |  | **86 / 100** |  |  |
