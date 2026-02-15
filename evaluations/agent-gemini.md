# Evaluation: agent/gemini

**Total Score: 28/100**
**Source:** 935 LOC | **Tests:** 29 LOC (3 passing: 1 unit + 2 integration)

---

## 1. Spec Compliance — 2/10 (×2 = 4/20)

**What matches the spec:**
- **Wire framing (u32 big-endian length + JSON)**: `send_envelope()` writes `len.to_be_bytes()` then JSON bytes (`transport.rs:79-85`). `recv_envelope()` reads 4 bytes, parses BE length, then reads exactly `len` bytes (`transport.rs:88-101`).
- **Max message size check (64KB-ish)**: rejects `len > 65536` on receive (`transport.rs:93-95`).
- **Envelope fields roughly match** the spec shape: includes `v, id, from, to, ts, kind, ref, payload` (`message.rs:6-16`), with `ref` mapped via `#[serde(rename="ref")]`.
- **Identity derivation** matches spec formula: SHA-256(pubkey) and first 16 bytes hex-encoded (`identity.rs:58-64`).
- **Identity key files & permissions**: writes `~/.axon/identity.key`, chmod 600 on Unix (`identity.rs:30-44`), writes base64 public key to `identity.pub` (`identity.rs:46-49`).
- **Discovery abstraction exists** with `PeerEvent::{Discovered, Lost}` (`discovery.rs:7-17`) and `Discovery` trait (`discovery.rs:19-21`), plus MdnsDiscovery and StaticDiscovery implementations (`discovery.rs:23-86`, `discovery.rs:88-113`).
- **IPC path and stale socket cleanup**: removes existing socket file then binds `~/.axon/axon.sock` (`ipc.rs:40-45`), and daemon also removes socket on shutdown (`daemon.rs:115-117`).

**Major spec mismatches / missing requirements:**
- **Message kinds and payload schemas are not implemented** beyond a generic `kind: String` and `payload: serde_json::Value` (`message.rs:12,15-16`). Only `HelloPayload` and `HelloResponsePayload` exist as structs (`message.rs:36-49`), but no structured handling for ping/pong, query/response, delegate/ack/result, notify, cancel, discover/capabilities, error.
- **Hello handshake is not spec-compliant and likely non-functional**: always opens a new bidirectional stream and sends hello (`daemon.rs:237-251`), even on inbound connections (should accept remote's stream). Sets `to: "unknown"` instead of the real peer (`daemon.rs:239-243`). Features advertised are incomplete: only `["ping","query"]` (`daemon.rs:243-247`), omitting required `notify`, `error`.
- **Stream mapping is not faithfully followed**: IPC send path decides bidir vs unidir by kind (`daemon.rs:192-203`), but does not wait for a response on bidirectional flows (explicit TODO) (`daemon.rs:197-200`).
- **TLS peer authentication is missing**: `PeerVerifier::verify_server_cert` returns `Ok(ServerCertVerified::assertion())` unconditionally (`transport.rs:63-76`).
- **Self-signed cert generation is likely incorrect**: uses `KeyPair::from_der(&self.signing_key.to_bytes())` (`identity.rs:76-78`), but `SigningKey::to_bytes()` is raw Ed25519 secret bytes, not a DER-encoded keypair.
- **Discovery never emits `PeerEvent::Lost`** from mDNS browsing; only handles `ServiceResolved` (`discovery.rs:61-82`).
- **IPC protocol is incomplete**: no registry of connected IPC clients; inbound QUIC messages are only logged (`daemon.rs:261-272`). IPC responses don't match spec's `status` shape (`daemon.rs:180-187`).
- **CLI is incomplete**: Only `daemon/send/peers/status/identity` exist (`main.rs:18-30`), missing `delegate`, `notify`, and others.
- **Connection lifecycle missing**: no keepalive/idle timeout config, no reconnect with exponential backoff, no 0-RTT handling, no "lower agent_id initiates" guard beyond a simple string compare.

---

## 2. Correctness — 2/10

- **Hello handshake deadlock / protocol breakage is very likely**: `handle_connection()` opens a bidirectional stream and waits for a response (`daemon.rs:237-252`). On the remote side, `handle_connection()` also opens its own `open_bi()` and waits, while the code that would accept remote streams only runs after handshake completes (`daemon.rs:261-272`). Both sides can end up waiting for a response that never arrives.
- **Certificate generation likely fails at runtime** due to `KeyPair::from_der` fed raw secret bytes (`identity.rs:76-78`).
- **Request/response is not implemented**: even when IPC opens bidirectional stream for `query`, it never reads the response (`daemon.rs:197-200`).
- **Inbound QUIC messages are dropped on the floor** (only logged) (`daemon.rs:265-272`).

---

## 3. Code Quality — 5/10

**Positive:**
- Modules are separated sensibly (`lib.rs:1-8`), with clear roles.
- Uses `anyhow::Result` consistently across async boundaries.
- Uses `tokio::sync::RwLock` for shared peer table and connection map (`peer.rs:4-5`, `daemon.rs:4,23-24`).

**Issues:**
- Overuse of untyped strings/JSON reduces correctness: `Envelope.kind: String` and `payload: serde_json::Value` (`message.rs:12,15-16`).
- Correctness-critical comment admits missing security logic: "For now, assertion passed" in cert verifier (`transport.rs:73-76`).
- Panic risks: `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()` (`message.rs:25-28`), `dirs::home_dir().expect(...)` (`main.rs:37`).

---

## 4. Test Coverage — 2/10

- Only three tiny tests exist:
  - Identity test checks derived ID length is 32 chars, not correctness vs known vectors (`identity.rs:88-99`).
  - Envelope JSON round-trip verifies only `v` and `kind` (`wire_compatibility.rs:4-22`).
  - Big-endian framing test checks `to_be_bytes` output for a constant (`wire_compatibility.rs:24-29`).
- No tests cover: QUIC handshake/hello, cert generation, cert validation, IPC protocol, peer table operations.

---

## 5. Security — 2/10

**Good:**
- Private key file permissions set to `0o600` on Unix (`identity.rs:40-44`).

**Critical gaps:**
- TLS peer authentication is effectively disabled (`transport.rs:63-76`).
- Potentially sensitive payload logging: logs full received envelopes with `{:?}` (`daemon.rs:266-272`).
- Unix socket permissions not explicitly locked down (`ipc.rs:40-45`).

---

## 6. Concurrency & Async Design — 4/10

**Strengths:**
- Uses `tokio::select!` to multiplex peer events, IPC accept, QUIC accept, shutdown (`daemon.rs:80-112`).
- Uses `Arc<RwLock<...>>` for shared state and spawns per-client/per-connection tasks.

**Weaknesses:**
- No structured cancellation/shutdown propagation to spawned tasks.
- Connection handler loops forever until `conn.closed()` triggers (`daemon.rs:261-277`), but no daemon-driven close is implemented.
- No keepalive timers / idle timeouts configured per spec.

---

## 7. Error Handling — 4/10

- Some boundary errors are propagated via `Result` (framing errors and oversize checks in `recv_envelope()`).
- No `error` message kind implementation at all.
- TODOs left in core protocol path (`daemon.rs:197-200`).
- Multiple panics/unwrap/expect in production code.

---

## 8. Completeness — 2/10

Large portions of the spec are missing or stubbed:
- Missing message kinds / handlers: ping/pong, response, delegate/ack/result, notify (as a handled inbound type), cancel, discover/capabilities, error.
- Missing required behaviors: hello negotiation, feature gating, cert validation, reconnect backoff, keepalive/idle, peer stale removal, known_peers periodic save, IPC inbound broadcast, CLI commands beyond basics.

---

## 9. Production Readiness — 3/10

**What's present:**
- Basic logging via `tracing` initialized in `main` (`main.rs:34`).
- Config file loading exists (`config.rs:26-38`) and is used for port override/static peers.
- Known peers load/save exists (`peer.rs:69-102`).

**What blocks real deployment:**
- Handshake likely broken (`daemon.rs:237-252` plus accept loop structure).
- No routing from QUIC to IPC (only logs).
- No reconnect/keepalive/idle timeout settings.
- Status reporting is placeholder.
- No robust security verification.

---

## Key Strengths
- Correct agent_id derivation implementation (`identity.rs:58-64`) and correct key storage locations.
- Correct wire framing implementation and receive-side size check (`transport.rs:79-85`, `transport.rs:88-101`).
- Basic scaffolding for daemon/discovery/IPC/transport exists with plausible module boundaries.

## Key Weaknesses
- Security is fundamentally incomplete: TLS cert verification is a no-op.
- Hello handshake implementation is likely deadlocking / incorrect and violates the spec semantics.
- Protocol completeness is very low: most message kinds are unimplemented, no response handling, no IPC inbound broadcast.
- CLI and IPC are incomplete vs spec.
