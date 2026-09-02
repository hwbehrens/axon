# Capability Manifests: `describe`, `who_can`, and Manifest Registration

Status: Implemented (Phases 1–2 shipped; Phase 3 deferred; Phase 4 follow-up)
Author: Hans + Codex (agent), 2026-02-17
Supersedes: nothing. Related: `plans/llm-friendliness-remediation.md` (archived).

## Problem

AXON solves *identity* discovery (who exists, is it really them) but not
*capability* discovery (what can a peer do, and what payload does it accept?).
An agent that connects to a peer has a socket and zero vocabulary: the true
"shout into the void" happens after a successful connection. For a densely
connected LAN of specialized agents, the cost of learning "who can do X and
how do I ask" must be O(1), not O(N) probes.

## Design (agreed in thread, 2026-02-17)

Disclosure ladder — nothing above tier 1 grants authority:

| Tier | Channel | Learned | Trust |
|---|---|---|---|
| 0 Broadcast | mDNS | exists, agent_id, pubkey, locator | untrusted hint |
| 1 Connected | QUIC post-handshake | version, uptime | authenticated identity |
| 2 Described | `describe` request | services, examples, limits | **self-reported claim** |
| 3 Exercised | actual exchange | whether claims are true | only verification |

Core decisions:

1. **`describe` becomes the fifth known message kind** (`request`-like,
   bidirectional). The *receiving daemon* answers it from a manifest cached at
   `serve` time; the IPC handler is never woken. The daemon is a cache and
   router, never the manifest's author.
   - Alternative rejected: payload-level `op` convention on `request`. It
     requires the daemon to inspect payloads (violates "payloads are opaque")
     or couples directory queries to handler liveness and forces every app to
     re-implement describe boilerplate it already published.
   - Compat: older peers treat `describe` as an unknown string; per
     MESSAGE_TYPES §Forward Compatibility a bidirectional unknown kind earns
     an `unsupported_kind` error that names the kind — instructive, graceful.
2. **Manifests are claims.** They never affect TLS trust, pinning, or
   enrollment. Serving one is opt-in; absence yields an explicit `no_manifest`
   error, never silence.
3. **`who_can` is a derived, cached view** over *connected enrolled peers*,
   refreshed by daemon-issued `describe` pulls (TTL-gated). It introduces no
   new authority and no durable store: cache entries are runtime-only and age
   out.
4. **Referrals stay tokens.** No gossip, no transitive trust, no reputation.
   `add_peer` with a token already covers "A vouches for C" out of band.

## Phases

### Phase 1 — manifest module ✅ shipped
- `axon/src/manifest/` directory module: `mod.rs`, `types.rs`, `cache.rs`,
  `types_tests.rs`, `cache_tests.rs`, nested `AGENTS.md`.
- `Manifest` / `ServiceEntry` serde types, validation, encoded-size bound,
  `ManifestCache` (bounded, TTL, insertion-order eviction).
- Bounds: manifest ≤ 32 KiB encoded; ≤ 64 services; string limits on
  `id` (≤128 B), `description` (≤2048 B), `errors` (≤32 × ≤64 B).
- Deviation from plan: the encoded-size bound is enforced inside
  `Manifest::from_parts` (i.e. at deserialization), not at dispatch. A parsed
  manifest therefore always satisfies every daemon invariant — on the serve
  path AND on remote describe-response parsing — which also makes the fuzz
  target's post-parse assertions valid. `daemon::command_handler::validate_manifest`
  was removed as redundant; serde failures surface as `invalid_command`.

### Phase 2 — wire + broker + IPC ✅ shipped
- `message/envelope.rs`: `MessageKind::Describe` (bidirectional; expects a
  response; never allowed on unidirectional streams).
- `request_broker`: `register(client_id, manifest)` atomically stores the
  handler's manifest with the lease; `describe.rs` submodule answers
  `describe` *before* handler lookup and before the completed-response cache.
  No manifest → `no_manifest` error envelope. Lease loss clears the manifest.
- IPC: `serve` accepts optional `manifest` (parse-validated →
  `invalid_command` on failure); `send` accepts `kind:"describe"` (timeout
  rules as for `request`); new `who_can` command (case-insensitive substring
  over service id + description; absent/whitespace query lists everything);
  `peers` summary gains advisory `services: [ids...]` from the cache.
  `who_can` reply carries `matches` and `unreachable`.
- Constants: `WHO_CAN_PULL_TIMEOUT` (5s), `WHO_CAN_CACHE_TTL` (60s),
  `MAX_MANIFEST_CACHE_ENTRIES` (256) — README Configuration Reference rows
  added.
- Fuzz target `fuzz_manifest` added (house rule: one per new deserialization
  entrypoint); asserts post-parse invariants (service bounds, re-encode, size).

### Phase 3 — candidate naming (deferred)
- `peer_candidate` events carrying mDNS-derived `display_name` (pass-through,
  untrusted). Discovery already tracks display names; this is surfacing only.
  Deferred to keep this change reviewable; no spec text shipped for it.

### Phase 4 — follow-ups (not in this change)
- CLI subcommands (`axon who-can`, manifest file for `axon serve`); raw IPC
  remains the canonical surface.
- `peer_candidate` role tags from a `role` mDNS TXT key.
- Annotated examples updated (serve-with-manifest, describe, who_can sections
  added) — this DID ship with Phase 2.

## Spec co-changes (same change, normative)

- `spec/MESSAGE_TYPES.md`: 4→5 kinds, describe row, daemon-answered rule,
  `no_manifest` error code, learnability item 3 rewording.
- `spec/WIRE_FORMAT.md`: describe envelope, manifest payload schema + bounds.
- `spec/IPC.md`: §4.1 kind list, §4.2 `services`, §4.7 `serve` manifest,
  new `who_can` section.
- `spec/SPEC.md`: capability-manifest paragraph in the messaging section.
- `README.md`: configuration reference rows for new constants.
- `docs/decision-log.md`: decision entry (4→5 kinds; broker answers describe;
  manifests are claims; who_can is a derived view).
- `AGENTS.md`, `CONTRIBUTING.md`, `docs/agent-index.json`, `llms.txt`,
  `axon/src/manifest/AGENTS.md`: module map / index rows ("four kinds"
  phrasing updated).

## Invariants touched

- Ownership: RequestBroker owns request completion — describe answering lives
  in the broker, not transport, not IPC. PeerDirectory/ConnectionManager
  untouched except for read-only connectivity queries.
- Payloads remain opaque to the *forwarding* path; describe is special-cased
  by kind, never by payload inspection.
- No durable state: manifests live in RAM, keyed by lease (local) and
  connection-scoped pulls (remote), TTL-expired.

## Verification

```sh
cd axon && make verify   # fmt + clippy -D warnings + full test suite
```

New tests: manifest parse/validate/round-trip/oversize; broker describe
answering (with/without manifest, no-handler, before replay cache, refreshed
manifest, cleared on disconnect); IPC protocol parsing (`serve` manifest,
`who_can`, `kind:"describe"`, services omission); who_can matching; e2e
two-daemon describe/who_can/peers round-trip (tests/daemon_e2e/capabilities.rs).
All shipped. Verification: `make verify` green (fmt, clippy -D warnings, full
suite incl. spec-compliance, adversarial, e2e).
