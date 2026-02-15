# Evaluation: agent/amp

**Total Score: 81/100**
**Source:** 6199 LOC | **Tests:** 174 tests (106 unit + 22 integration + 46 spec compliance), all passing

---

## 1. Spec Compliance — 9/10 (×2 = 18/20)

**What matches well:**
- **Identity derivation matches spec (Ed25519 → agent_id = SHA-256(pubkey)[0..16] hex):** `derive_agent_id()` in `identity.rs:126-132`.
- **Private key persisted with correct permissions (chmod 600):** `fs::set_permissions(... 0o600)` in `identity.rs:47-50`, with test in `identity.rs:174-182`.
- **Ephemeral self-signed cert derived from same Ed25519 key:** `make_quic_certificate()` in `identity.rs:81-103` (rcgen `PKCS_ED25519`), and cert pubkey extraction validated in integration test `tests/integration.rs:70-82`.
- **Discovery trait + PeerEvent + MdnsDiscovery + StaticDiscovery present:** `PeerEvent` + `Discovery` in `discovery.rs:14-29`; `StaticDiscovery` in `discovery.rs:35-61`; `MdnsDiscovery` in `discovery.rs:67-155`.
- **mDNS service type + TXT records match spec:** `_axon._udp.local.` in `discovery.rs:12`; TXT props `agent_id` / `pubkey` in `discovery.rs:91-95`.
- **Wire framing matches spec (u32 big-endian length prefix + JSON):** `encode()` in `message.rs:162-172`; spec test `wire_format_length_prefix_is_big_endian_u32` in `tests/spec_compliance.rs:607-615`.
- **Max message size 64KB enforced:** `MAX_MESSAGE_SIZE = 65536` in `message.rs:10`; enforced in `encode()` (`message.rs:165-167`) and in QUIC read framing (`transport.rs:636-641`).
- **Envelope fields align with spec (v,id,from,to,ts,kind,ref,payload):** `Envelope` in `message.rs:99-110`, including `#[serde(rename="ref"...)]` for `ref_id` at `message.rs:107-109`.
- **All message kinds from message-types.md modeled:** `MessageKind` includes `hello/ping/pong/query/response/delegate/ack/result/notify/cancel/discover/capabilities/error` in `message.rs:18-34`.
- **Stream mapping implemented (bidir if expects_response else unidir):** `MessageKind::expects_response()` in `message.rs:37-47` and `QuicTransport::send()` in `transport.rs:119-129`.
- **Hello handshake implemented for outbound connections:** `perform_hello()` in `transport.rs:143-194`, including version negotiation checks.
- **Connection lifecycle pieces present per spec:**
  - Keepalive/idle timeout constants match spec: `KEEPALIVE_INTERVAL = 15s`, `IDLE_TIMEOUT = 60s` in `transport.rs:24-26`, wired into quinn config at `transport.rs:282-289`.
  - **Reconnect with exponential backoff (1s → 30s cap):** `ReconnectState` in `daemon.rs:40-60` and reconnect loop in `daemon.rs:339-412`.
  - **Deterministic initiator rule (lower agent_id initiates) used for reconnection scheduling:** `if local_agent_id < peer.agent_id { reconnect_state.insert(...) }` in `daemon.rs:193-197`.
- **IPC matches spec requirements:**
  - Socket path default `~/.axon/axon.sock`: `AxonPaths::discover()` in `config.rs:21-25`.
  - Line-delimited JSON commands with `send/peers/status`: `IpcCommand` in `ipc.rs:22-34`.
  - Multiple IPC clients supported + inbound broadcast to all clients: `broadcast_inbound()` in `ipc.rs:140-150`.
  - Stale socket cleanup + socket permissions set to user-only: `remove_file` in `ipc.rs:93-100` and `set_permissions(...0o600)` in `ipc.rs:109-118`.
- **CLI commands required by spec are present (daemon/send/delegate/notify/peers/status/identity + discover, ping, cancel, examples):** `Commands` enum in `main.rs:23-71`, with cancel support in `main.rs:55-62`.

**Spec deviations / gaps:**
- **Lower-agent-id initiates is not enforced for "on-demand send" paths.** Reconnection scheduling honors it (`daemon.rs:193-197`), but `handle_command` calls `transport.send()` for any target peer (`daemon.rs:364-391`), which calls `ensure_connection()` (`transport.rs:77-117`) without checking the initiator rule.
- **`--agent-id <override>` CLI option from spec is not implemented.** The daemon CLI supports `--port` and `--enable-mdns` only (`main.rs:25-30`).

---

## 2. Correctness — 8/10

**What's correct:**
1. **Static peers are protected from being overwritten by discovery refreshes.** `PeerTable::upsert_discovered()` only overwrites addr/pubkey/source if the existing record is not Static (`peer_table.rs:94-100`). This prevents static peers from being converted into "discovered" and then removed by stale cleanup.
2. **mDNS "Lost" events correctly map to full agent IDs.** The implementation stores `fullname → agent_id` on resolve (`discovery.rs:113-133`) and uses that map for removals (`discovery.rs:139-145`), rather than attempting to reconstruct IDs from instance name strings.
3. **Envelope validation is applied at the IPC boundary.** `envelope.validate()` is checked before sending (`daemon.rs:460-469`), preventing malformed from/to/timestamps from being emitted.
4. **Replay protection mitigates 0-RTT replay concerns.** `ReplayCache` checks message IDs with TTL and drops replays (`daemon.rs:62-87`, used for inbound at `daemon.rs:144-147` and for responses at `daemon.rs:482-484`).

**Remaining correctness concerns:**
- **Transport authentication depends on the expected_pubkeys map being populated in time.** TLS verifiers check against `expected_pubkeys` if present (`transport.rs:744-755`), but if a peer connects inbound before being inserted into the map, the verifier may accept it (TOFU behavior). Aligned with spec's TOFU discussion but worth noting.
- **Graceful shutdown sequence calls helpers** (`transport.close_all()` at `transport.rs:131-135`, `ipc.cleanup_socket()` at `ipc.rs:160-170`, `save_known_peers` at `daemon.rs:274-276`) but spawned background tasks (inbound forwarder, discovery loops) lack explicit cancellation tokens.

---

## 3. Code Quality — 8/10

**Strengths:**
- **Good module boundaries and separation of concerns:** identity/config/message/discovery/peer_table/ipc/transport/daemon are all clearly separated (`lib.rs:1-8`).
- **Idiomatic serde modeling for the protocol:** `Envelope` and `MessageKind` are cleanly represented (`message.rs:18-110`), and payload structs use sensible `#[serde(default)]` patterns (e.g., `DelegatePayload` defaults in `message.rs:234-249`).
- **Reasonable transport API layering:** high-level `QuicTransport::send()` decides request vs unidir (`transport.rs:119-129`), while `ensure_connection()` encapsulates handshake + hello (`transport.rs:77-117`).
- **Uses `anyhow::Context` extensively** for descriptive error wrapping throughout.

**Issues:**
- **Mixing std locks in async-heavy code paths:** `expected_pubkeys` uses `std::sync::RwLock` (`transport.rs:3-4`, field at `transport.rs:41`), and replay cache uses `std::sync::Mutex` (`daemon.rs:4-5`, `daemon.rs:62-65`). These are short critical sections, but an avoidable footgun in tokio contexts.
- Some error strings are fairly generic (often `err.to_string()` surfaced directly to IPC clients in `daemon.rs:490-498`).

---

## 4. Test Coverage — 9/10

**What's strong:**
- **Broad protocol compliance test suite** directly tied to spec requirements (`tests/spec_compliance.rs`), covering:
  - Envelope field presence and semantics (`spec_compliance.rs:29-65`)
  - Forward compatibility/unknown fields (`spec_compliance.rs:94-110`)
  - Payload schema matches for all kinds (`spec_compliance.rs:116-352`)
  - Wire framing + size bounds (`spec_compliance.rs:606-650`)
  - Identity invariants (`spec_compliance.rs:656-676`)
  - IPC shapes (`spec_compliance.rs:682-738`)
  - Static peers config parsing (`spec_compliance.rs:744-766`)
- **Unit tests for critical edge cases** like invalid key length handling (`identity.rs:207-220`), socket cleanup/permissions and multi-client broadcast (`ipc.rs:399-558`), peer table concurrency (`peer_table.rs:452-482`).
- **Integration tests cover cross-module invariants:** identity/cert pubkey match, config persistence, wire format round-trips for all message kinds, QUIC hello/query/delegate/cancel/notify/discover exchanges, IPC command round-trips (`tests/integration.rs`).

**Gaps:**
- No tests for long-running daemon lifecycle behaviors (e.g., graceful shutdown closing QUIC and removing socket; reconnect succeeding after peer restarts).

---

## 5. Security — 8/10

**What's good:**
- **Private key and AXON root directory permissions locked down:** identity key `0o600` (`identity.rs:47-50`) and root dir `0o700` (`config.rs:44-49`).
- **Unix socket permissions explicitly set to user-only:** `fs::set_permissions(...0o600)` in `ipc.rs:109-118`.
- **TLS peer verification ties certificate pubkey → agent_id and (optionally) discovery pubkey:**
  - Server verifier checks derived agent_id matches SNI + checks expected pubkey if known (`transport.rs:714-758`).
  - Client verifier checks expected pubkey if known (`transport.rs:761-791`).
- **Hello identity validation at application layer:** `validate_hello_identity()` verifies cert pubkey matches `from` field and matches expected discovery pubkey (`transport.rs:435-458`).
- **Max message size enforcement** prevents trivial memory DoS on framing (`message.rs:165-167`, `transport.rs:636-641`).

**Concerns:**
- **TOFU surface remains for first-contact inbound connections** if `expected_pubkeys` has no entry (verifiers only enforce pubkey match *if present*). Consistent with spec's TOFU discussion but a real security trade-off.
- **Replay cache is daemon-local and time-based** (`daemon.rs:62-87`); it mitigates but does not eliminate replay risks across restarts.

---

## 6. Concurrency & Async Design — 7/10

**Good patterns:**
- **Tokio tasks used for independent subsystems:** inbound forwarder (`daemon.rs:140-164`), discovery tasks (`daemon.rs:168-189`), and transport accept loop (`transport.rs:196-228`).
- **Per-peer lock prevents duplicate concurrent connects:** `connecting_locks` + `Mutex<()>` guard in `transport.rs:39-43` and `transport.rs:85-98`.
- **Broadcast channel for inbound message fanout** is a good fit (`broadcast::Sender<Envelope>` at `transport.rs:42`, consumed in daemon at `daemon.rs:135-164`).
- **Double-checked locking pattern** for connection establishment prevents redundant QUIC handshakes (`transport.rs:80-98`).

**Issues:**
- **Use of std locks in async contexts** can block executor threads in worst cases (see Code Quality).
- **Shutdown is signal-driven but not structured cancellation.** The daemon breaks on ctrl-c (`daemon.rs:218-221`) and calls cleanup helpers, but spawned long-running tasks (inbound forwarder, mdns browse loop, IPC accept loop) don't have explicit cancellation wiring.

---

## 7. Error Handling — 7/10

**Strengths:**
- **Validation errors surfaced cleanly to IPC clients** as `{ok:false,error:...}`: e.g., invalid envelope results in structured error reply (`daemon.rs:460-469`).
- **IPC parsing errors return structured error without crashing:** `invalid command: {err}` (`ipc.rs:233-241`).
- **Hello negotiation handles incompatible versions via `error` response kind:** `auto_response()` returns `incompatible_version` error for unsupported protocol versions (`transport.rs:466-482`).
- **No panics in non-test code** (uses `anyhow::Result` throughout, with no visible `unwrap()` or `expect()` in daemon/transport/IPC paths).

**Weak spots:**
- **Not consistently "instructive errors" per spec.** Many operational failures are returned as raw error strings (`DaemonReply::Error { error: err.to_string() }` in `daemon.rs:490-498`). The spec guidance calls for error messages that suggest what to do next.
- **`now_millis()` uses `unwrap_or_default()`** (`message.rs:179-184`), which avoids panics but can yield `ts=0` in pathological cases; the resulting validation error from `Envelope::validate()` might be surprising.

---

## 8. Completeness — 9/10

**Implemented end-to-end:**
- Identity/key/cert, config + static peers, known peers cache load, QUIC transport, hello handshake with version negotiation, discovery (static + mdns), IPC server with multi-client broadcast, CLI command suite (daemon/send/delegate/notify/ping/discover/cancel/peers/status/identity/examples), reconnection with exponential backoff, stale discovery removal, message framing and max-size enforcement, replay deduplication, peer table lifecycle.

**Remaining missing items:**
- `--agent-id` override CLI option (spec'd but not present in `main.rs:25-30`).
- `axon examples` only prints text guidance; spec envisions a complete annotated interaction sequence.

---

## 9. Production Readiness — 7/10

**Pros:**
- **Operational logging:** daemon startup and warnings/errors use `tracing` macros (`daemon.rs:9, 106-107, 145-146`), and CLI initializes tracing subscriber with env filter (`main.rs:199-202`).
- **Reconnection loop with bounded exponential backoff** runs periodically (`daemon.rs:199-203`, `daemon.rs:339-412`).
- **Known peers persistence:** load at startup (`daemon.rs:113-115`), save on peer events (`daemon.rs:238-241`), periodic save interval (`daemon.rs:263-267`), and save on shutdown (`daemon.rs:274-276`).
- **Config file support** with defaults (`config.rs:62-77`).

**Cons:**
- **Graceful shutdown sequence is present but may leave background tasks running.** The daemon breaks, calls `close_all()` and `cleanup_socket()` (`daemon.rs:272-278`), but spawned tasks (inbound forwarder, discovery, IPC accept) are not explicitly cancelled.
- **Initiator rule ambiguity** for on-demand sends may cause duplicate connections in real-world operation.

---

## Summary Table

| Category | Weight | Score |
|---|---|---|
| Spec Compliance | ×2 | 18/20 |
| Correctness | ×1 | 8/10 |
| Code Quality | ×1 | 8/10 |
| Test Coverage | ×1 | 9/10 |
| Security | ×1 | 8/10 |
| Concurrency & Async | ×1 | 7/10 |
| Error Handling | ×1 | 7/10 |
| Completeness | ×1 | 9/10 |
| Production Readiness | ×1 | 7/10 |
| **Total** | | **81/100** |

## Key Strengths
- Strong spec alignment on identity, envelope/wire framing, IPC protocol shapes, and message kind coverage.
- Meaningful security: strict TLS verification based on agent_id derived from cert pubkey + optional match against discovery pubkey, plus correct file/socket permissions.
- Reconnect/backoff and replay-dedup are implemented and tested.
- Very strong automated test suite breadth (174 passing tests), including explicit spec compliance tests.
- Core correctness bugs from prior version fixed: static peers now protected from stale removal, mDNS lost events properly mapped.

## Key Weaknesses
- Deterministic "lower agent_id initiates connection" is implemented for reconnection but not enforced for ad-hoc sends.
- Instructive error messaging (per message-types spec guidance) is only partially realized.
- Graceful shutdown calls cleanup helpers but doesn't cancel spawned background tasks.
- `--agent-id` CLI override not implemented.
