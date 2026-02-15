# Evaluation: agent/codex

**Total Score: 69/100**
**Source:** 3239 LOC | **Tests:** 442 LOC (32 passing: 30 unit + 2 integration, 2 ignored e2e)

---

## 1. Spec Compliance — 6.5/10 (×2 = 13/20)

**What matches the spec:**
- **Envelope shape is correct**: `v`, `id`, `from`, `to`, `ts`, `kind`, `ref`, `payload` implemented as `Envelope` with `ref` serialized via `#[serde(rename="ref")]` (`message.rs:45-56`).
- **All message kinds exist** (`hello/ping/pong/query/response/delegate/ack/result/notify/cancel/discover/capabilities/error`) in `MessageKind` (`message.rs:13-29`).
- **Stream mapping is implemented correctly**: messages where `expects_response()` is true use bidirectional streams, otherwise uni (`message.rs:31-43`; `transport.rs:94-100`).
- **Wire framing**: u32 big-endian length prefix + JSON implemented by `write_framed()` and `read_framed()` (`transport.rs:246-279`).
- **Max message size (64KB)**: constant `MAX_MESSAGE_SIZE` is `64 * 1024` (`message.rs:8-10`) and enforced for outbound (`transport.rs:175-180`, `transport.rs:201-206`) and inbound reads (`transport.rs:262-272`).
- **Identity derivation matches spec**: agent id = first 16 bytes of SHA-256(pubkey) hex-encoded (`identity.rs:141-147`). Private key stored with `0o600` perms (`identity.rs:52-58`); pubkey stored base64 (`identity.rs:62-69`).
- **Self-signed certificate via rcgen from the Ed25519 key**: `make_quic_certificate()` constructs an rcgen keypair and serializes cert/key (`identity.rs:96-119`).
- **Discovery trait + PeerEvent enum exist; Static + mDNS implementations exist** (`discovery.rs:14-29`, `discovery.rs:31-56`, `discovery.rs:58-151`). Service type is `_axon._udp.local.` (`discovery.rs:12`) and TXT includes `agent_id` and `pubkey` (`discovery.rs:88-91`).
- **IPC Unix socket and line-delimited JSON**: server removes stale socket (`ipc.rs:82-90`), binds UnixListener (`ipc.rs:99-100`), sets socket perms `0o600` (`ipc.rs:101-108`), reads line-delimited commands via `BufReader::lines()` (`ipc.rs:214-232`), and broadcasts inbound to all clients (`ipc.rs:130-140`). Socket path matches `~/.axon/axon.sock` via `AxonPaths` (`config.rs:33`).
- **Deterministic "lower agent_id initiates"**: only peers with `local_agent_id < peer.agent_id` are scheduled for reconnect attempts (`daemon.rs:179-182`, `daemon.rs:296-300`).

**Spec mismatches / gaps:**
- **Hello payload schema deviation**: spec's `hello.payload` does not include `pubkey`, but this implementation injects `"pubkey"` into the `hello` payload (`transport.rs:114-126`). Tolerable but not faithful.
- **CLI command set incomplete**: no `Cancel` command — `main.rs:23-53`.
- **Keepalive/idle timeout settings not verifiable** from visible code.

---

## 2. Correctness — 7/10

**Evidence of correctness:**
- Core invariants are checked: envelope validation enforces non-zero version, 32-char IDs, non-zero timestamp (`message.rs:90-101`).
- QUIC framing validates declared size and exact length match (`transport.rs:265-278`).
- Integration test demonstrates two transports can exchange a `notify` end-to-end over QUIC (`message_routing.rs:22-75`).

**Correctness risks / bugs:**
- **Connection race / duplicate connects**: `ensure_connection()` checks `connections` under a read lock, then performs network connect, then inserts under a write lock (`transport.rs:68-87`). Two concurrent callers can both observe "no connection" and create parallel connections.
- **Panics in non-test code**: `now_millis()` uses `expect("system clock before epoch")` (`message.rs:105-108`). `PeerTable` uses `.expect("peer table lock poisoned")` repeatedly (`peer_table.rs:98-99`, `peer_table.rs:110-111`, etc.).
- **Hex validation is incomplete**: `Envelope::validate()` checks only length of agent IDs, not hex charset (`message.rs:94-96`).

---

## 3. Code Quality — 8/10

**Strengths:**
- Clear module separation (`lib.rs:1-8`), with coherent responsibilities.
- Mostly idiomatic error handling with `anyhow` + `Context` (`identity.rs:30-35`, `transport.rs:74-80`, `ipc.rs:84-90`).
- Message types use serde properly, including forward-compat "unknown fields ignored" test at the envelope level (`message.rs:206-220`).

**Weak spots:**
- Manual PKCS#8 DER construction for Ed25519 (`identity.rs:122-139`) is brittle.
- Mixed sync primitives inside async runtime (`PeerTable` uses `std::sync::RwLock` in `peer_table.rs:88-90`; `ReplayCache` uses `std::sync::Mutex` in `daemon.rs:41-43`).

---

## 4. Test Coverage — 7/10

**What's covered well:**
- Identity derivation and persistence round-trip (`identity.rs:157-180`), cert material existence (`identity.rs:182-195`).
- Envelope serde round-trip, unknown fields ignored, response `ref` linkage, `expects_response` mapping (`message.rs:191-251`).
- Config parsing + known peers persistence (`config.rs:124-168`).
- Peer table behaviors: upserts, stale removal, status transitions (`peer_table.rs:220-330`).
- Discovery parsing and StaticDiscovery emissions (`discovery.rs:230-265`, `discovery.rs:266-320`).
- IPC parsing, broadcast behavior, invalid command path (`ipc.rs:249-333`).
- **Integration tests**: QUIC notify routing across two endpoints (`message_routing.rs:22-75`). IPC status round-trip (`ipc_command_flow.rs:11-63`).

**Gaps:**
- No tests for cert verifier behavior rejecting mismatched pubkeys.
- E2E tests exist but are ignored (`e2e_localhost.rs:143-146`, `e2e_localhost.rs:244-246`).

---

## 5. Security — 8/10

**Strong security-aligned choices:**
- Private key permissions `0600` (`identity.rs:52-58`) and AXON dir permissions `0700` (`config.rs:44-49`).
- Unix socket permissions `0600` (`ipc.rs:101-108`) and stale socket removal on startup (`ipc.rs:82-90`).
- **TLS peer verification binds the QUIC cert key to discovery data**: Server cert verification checks expected pubkey by agent id and recomputes agent id from cert pubkey (`transport.rs:394-421`). Client cert verification derives agent id from cert key and requires it to be in `expected_pubkeys` and match (`transport.rs:349-370`).
- **Basic replay defense**: inbound messages dropped if UUID seen recently (`daemon.rs:39-60`, used at `daemon.rs:130-133`).

**Concerns:**
- Trust model depends on `expected_pubkeys` being populated correctly; first-contact risk remains (spec acknowledges TOFU).
- `Envelope::validate()` does not verify agent IDs are hex (`message.rs:94-96`).

---

## 6. Concurrency & Async Design — 6/10

**Good:**
- Uses `tokio::select!` main loop for daemon orchestration (`daemon.rs:186-248`).
- Broadcast channel for inbound envelopes enables fan-out to IPC layer (`transport.rs:30-31`, `transport.rs:51-53`).
- IPC supports multiple clients via a shared clients map + per-client writer task (`ipc.rs:75-80`, `ipc.rs:166-191`).

**Issues:**
- Sync locks in async contexts can block the runtime: `PeerTable` uses `std::sync::RwLock` (`peer_table.rs:88-90`), `ReplayCache` uses `std::sync::Mutex` (`daemon.rs:41-43`).
- Potential duplicate connections race (`transport.rs:68-87`).

---

## 7. Error Handling — 7/10

**Good:**
- Errors are generally propagated with context (`identity.rs:30-35`, `transport.rs:74-80`, `ipc.rs:84-100`).
- IPC invalid commands produce structured errors (`ipc.rs:223-230`).

**Weak:**
- Several `.expect(...)` on lock poisoning (`peer_table.rs:98-99` etc.) and time (`message.rs:105-108`) can crash the daemon in edge conditions.

---

## 8. Completeness — 7/10

**Implemented substantially:**
- Core identity + cert, discovery (mDNS + static), peer table, QUIC transport framing, IPC, daemon orchestration, persistence of known peers (`config.rs:94-117`; daemon periodic saves `daemon.rs:174-176`, `daemon.rs:242-245`).
- Many spec "quality of life" features: `axon examples` (`main.rs:52-53`, `main.rs:168-174`), cached peers file, reconnect backoff (`daemon.rs:63-77`).

**Missing / uncertain:**
- CLI missing `cancel` (`main.rs:23-53`).
- Full receive-loop handling for all message kinds not fully verifiable.

---

## 9. Production Readiness — 6/10

**Positives:**
- Uses `tracing` + `tracing-subscriber` (`main.rs:10-11`, `main.rs:180-183`).
- Configurable port via config + CLI override (`config.rs:74-77`; `daemon.rs:86-88`; `main.rs:25-32`).
- Graceful-ish shutdown path exists (ctrl-c breaks loop and `close_all()` is called) (`daemon.rs:184-191`, `daemon.rs:250`).
- Reconnect loop and stale peer cleanup loops exist.

**Negatives:**
- Keepalive/idle timeout settings cannot be confirmed from visible code.
- Panics on poisoned locks / system time issues are undesirable for a long-running daemon.
- Duplicate-connection race could cause real-world flakiness.

---

## Key Strengths
- **Best security posture of all implementations**: real TLS peer cert validation against discovery pubkeys, socket permissions, directory permissions, replay detection.
- Strong alignment on identity + agent_id derivation and filesystem permissions.
- Correct wire framing with size checks and max message size enforcement.
- Real, meaningful tests including QUIC routing integration and robust unit coverage across modules.

## Key Weaknesses
- Spec drift: extra `pubkey` field in hello payload and missing CLI cancel.
- Async design risks: sync locks inside async + connection race.
- Crash risk via `.expect()` in daemon-path code.
