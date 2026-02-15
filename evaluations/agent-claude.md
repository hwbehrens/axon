# Evaluation: agent/claude

**Total Score: 61/100**
**Source:** 3736 LOC | **Tests:** 842 LOC (71 passing: 59 unit + 12 integration, 3 ignored e2e)

---

## 1. Spec Compliance — 6/10 (×2 = 12/20)

**What matches the spec well:**
- **Envelope fields & naming**: `Envelope` includes `v`, `id`, `from`, `to`, `ts`, `kind`, `ref`, `payload` exactly as in spec — `message.rs:7-25`, with `ref` mapped via `#[serde(rename="ref")]` to `ref_id` — `message.rs:21-22`.
- **All message kinds present**: `MessageKind` includes `hello`, `ping`, `pong`, `query`, `response`, `delegate`, `ack`, `result`, `notify`, `cancel`, `discover`, `capabilities`, `error` — `message.rs:91-105`.
- **Hello exchange exists and is enforced as "first bidi"**: Initiator sends `hello` on a bidi stream and reads response — `transport.rs:197-246`. Responder detects `hello` on bidi, selects version, replies on same stream, and marks completion — `transport.rs:296-355`.
- **Lower agent_id initiates**: `connect_to_peer` returns early when local ID is higher/equal — `transport.rs:135-144`.
- **Keepalive/idle timeout**: QUIC idle timeout 60s and keepalive 15s are set — `transport.rs:30-35`.
- **Identity derivation & storage**: Agent ID = first 16 bytes SHA-256(pubkey), hex — `identity.rs:16-20`. Private key file permissions 0600 on Unix — `identity.rs:121-128`.
- **Self-signed X.509 cert generation via rcgen from Ed25519 key**: `generate_self_signed_cert` uses `rcgen` and returns DER cert/key — `identity.rs:140-166`.
- **Discovery trait + PeerEvent**: `Discovery` trait and `PeerEvent::{Discovered,Lost}` match the spec shape — `discovery.rs:7-26`.
- **mDNS service & TXT records**: advertises `_axon._udp.local.` — `discovery.rs:75`; sets TXT `agent_id` and `pubkey` — `discovery.rs:103-106`.
- **IPC wire format (line-delimited JSON)**: Reads lines with `BufReader::lines()` — `ipc.rs:151-155`, `ipc.rs:179-184`. Writes one JSON object per line with `\n` — `ipc.rs:168-175` and `ipc.rs:209-213`.
- **IPC commands implemented**: `send`, `peers`, `status` exist — `ipc.rs:13-24`.
- **Socket path**: daemon binds `~/.axon/axon.sock` — `daemon.rs:74-78`.
- **Reconnect with exponential backoff (max 30s)**: implemented in daemon loop — `daemon.rs:18-23`, `daemon.rs:350-355`.
- **CLI commands**: `daemon/send/delegate/notify/peers/status/identity` present — `cli.rs:22-101` and wired in `main.rs:15-207`. Also includes `discover` + `examples` — `cli.rs:102-119`, `main.rs:209-238`.

**Spec gaps / deviations:**
- **TLS peer authentication is not spec-compliant**: Server TLS config does not request client auth: `offer_client_auth()` returns `false` — `transport.rs:427-430`. Both client and server verifiers accept any certificate — `transport.rs:436-461`.
- **Max message size 64KB**: constant exists and is tested, but enforcement on read/write paths is not fully verifiable from visible code.
- **Config default port behavior is inconsistent**: `Config` derives `Default`, yielding `port = 0` — `config.rs:7-16`. Daemon patches `0` back to `7100` at runtime — `daemon.rs:44-46`.

---

## 2. Correctness — 6/10

- **High likelihood of incorrect/missing mutual authentication** due to `offer_client_auth() == false` on server — `transport.rs:427-430`.
- **Daemon runtime appears coherent and test-backed**: Main orchestration covers identity/config/peer cache/transport/ipc/discovery/startup connections/select-loop/shutdown — `daemon.rs:28-382`. Integration test demonstrates two transports can complete hello and deliver a `notify` — `integration.rs:233-293`.
- **Potential performance bug in dedup pruning**: `seen_messages_order.remove(0)` inside a loop is O(n) per prune — `daemon.rs:203-209`.

---

## 3. Code Quality — 7/10

- **Generally idiomatic structure**: clear modules (`lib.rs:1-9`), `anyhow::Result` + `.context(...)` used at boundaries (`identity.rs:88-89`, `config.rs:36-38`, `daemon.rs:31-36`, `ipc.rs:102-115`).
- **Good typed modeling for protocol payloads** (e.g., `HelloPayload`, `QueryPayload`, etc.) — `message.rs:160-219`.
- **One design smell**: `Config` uses derived `Default` but relies on runtime patching for port — `config.rs:55-60`. A manual `impl Default` with `port: 7100` would be safer.

---

## 4. Test Coverage — 7/10

**Strengths:**
- **Identity tests** include determinism, save/load, base64 pubkey roundtrip, and Unix permission check — `identity.rs:207-297`.
- **Message tests** cover envelope round-trips for multiple kinds (notify/error/ping-pong/ack/result/cancel/discover/capabilities) — `message.rs:254-389`. `ref` behavior in replies — `message.rs:391-409`. Length-prefix encode/decode — `message.rs:411-440`. Unknown fields are ignored — `message.rs:461-476`.
- **PeerTable tests** cover CRUD, stale removal rules, cache save/load, RTT, connected filter — `peer.rs:257-376`.
- **IPC tests** include real server/client roundtrip — `ipc.rs:375-437`.
- **Integration tests** check cross-module behavior including hello + notify delivery — `integration.rs:233-293`.

**Gaps:**
- No tests for query/response stream behavior, delegate/ack/result stream mapping, cancel/ack, discover/capabilities exchange.
- E2E tests are ignored — `e2e.rs:71-73`, `e2e.rs:171-173`, `e2e.rs:213-215`.

---

## 5. Security — 4/10

**Major issues:**
- **TLS layer accepts any certificate** (client and server) — `transport.rs:446-461` and `transport.rs:436-444`.
- **Server does not request client certificates** (`offer_client_auth` returns false) — `transport.rs:427-430`, undermining the spec's core mutual auth requirement.
- **Unix socket permissions are not explicitly set** after bind — `ipc.rs:113-115`.

**Good points:**
- Private key stored with 0600 perms — `identity.rs:121-128`.
- Stale socket cleanup on startup — `ipc.rs:103-106`.

---

## 6. Concurrency & Async Design — 6/10

- **Reasonable use of Tokio primitives**: Shared mutable maps guarded by `tokio::sync::Mutex`/`RwLock` — `transport.rs:20-27`, `peer.rs:10-11`. Broadcast inbound messages to multiple IPC clients — `ipc.rs:78-80`, `ipc.rs:156-176`.
- **Correctly isolates blocking mDNS receiver** with `spawn_blocking` and `blocking_send` — `discovery.rs:129-177`.
- **No explicit shutdown wiring** for long-running spawned tasks (IPC accept loop, mDNS loop) — `ipc.rs:120-139`, `discovery.rs:129-201`.

---

## 7. Error Handling — 7/10

- Strong boundary contexts on many I/O operations — `identity.rs:88-100`, `config.rs:36-43`, `daemon.rs:68-72`, `ipc.rs:102-115`, `main.rs:34-37`.
- IPC returns structured errors on invalid commands — `ipc.rs:185-198`.
- Some non-ideal `unwrap()` usage in non-test code: `build_transport_config` uses `try_into().unwrap()` — `transport.rs:32`.

---

## 8. Completeness — 6/10

**Implemented:**
- Identity persistence + agent id derivation — `identity.rs:59-135`.
- QUIC endpoint setup, accepting connections, hello exchange — `transport.rs:37-61`, `transport.rs:91-133`, `transport.rs:197-277`, `transport.rs:296-355`.
- Discovery: static + mDNS — `discovery.rs:28-205`.
- Peer table + cache read/write — `peer.rs:58-224`.
- IPC server/client + daemon orchestration — `ipc.rs:73-267`, `daemon.rs:28-382`.
- CLI — `cli.rs:1-173`, `main.rs:11-240`.

**Not implemented / not verifiable:**
- Transport send/receive details for non-hello request/response patterns (query/response, delegate/ack/result) not fully verifiable.
- Cert validation behavior for inbound connections not fully verifiable.

---

## 9. Production Readiness — 6/10

- **Logging/tracing is present** — `main.rs:17-24`; daemon/transport/discovery log key lifecycle events.
- **Graceful shutdown exists**: on SIGINT/SIGTERM breaks loop and closes QUIC connections + endpoint — `daemon.rs:105-110`, `daemon.rs:362-380`, `transport.rs:400-411`.
- **Known peers cache persistence** is implemented and saved periodically — `daemon.rs:114-127`, `peer.rs:158-176`.
- **Unix-only signal handling** is unguarded (`tokio::signal::unix`) — `daemon.rs:105-110`.
- Socket permission hardening missing — `ipc.rs:113-115`.

---

## Key Strengths
- Strong protocol modeling and serde forward-compat behavior (unknown fields ignored) — `message.rs:461-476`; full kind coverage — `message.rs:91-105`.
- Identity meets spec and enforces key permissions on Unix — `identity.rs:16-20`, `identity.rs:121-128`.
- Practical daemon orchestration with reconnection backoff and periodic stale-peer cleanup — `daemon.rs:111-129`, `daemon.rs:307-360`.
- IPC supports multiple clients and broadcasts inbound envelopes — `ipc.rs:78-80`, `ipc.rs:156-176`, `daemon.rs:225-230`.

## Key Weaknesses
- TLS authentication is likely incorrect/incomplete: server does not request client certificates, TLS verifiers accept any cert.
- Config default behavior is confusing/unsafe (`Default` yields port 0; daemon patches later).
- Several spec-critical transport behaviors not fully verifiable due to code truncation during review.
