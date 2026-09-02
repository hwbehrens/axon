# AXON — Agent eXchange Over Network

Status: Normative

LLM-first local messaging protocol + Rust daemon/CLI for secure agent-to-agent communication over QUIC.

## Status

Working implementation. The Rust crate in `axon/` includes the daemon, CLI, IPC, QUIC transport, intentional peer enrollment, Bonjour/mDNS candidate discovery, and a full test/fuzz/bench harness. Specs in `spec/` are authoritative; if implementation disagrees, the spec wins.

## Document Authority

### Status taxonomy

| Status | Meaning |
|---|---|
| `Normative` | Binding source of truth; implementation must match. |
| `Draft` | Design guidance; not binding. |
| `Archived` | Historical context only; not binding. |

### Authority hierarchy

`spec/*` > `AGENTS.md` / `CONTRIBUTING.md` > `README.md` > code comments.

### Escalation rule

If two normative sources conflict or implementation behavior disagrees with a spec, **stop and request clarification** before proceeding. Do not silently choose one interpretation. Log the conflict in `docs/open-questions.md`.

### Status matrix

| Document | Status |
|---|---|
| `spec/SPEC.md` | Normative |
| `spec/MESSAGE_TYPES.md` | Normative |
| `spec/WIRE_FORMAT.md` | Normative |
| `spec/IPC.md` | Normative |
| `AGENTS.md` | Normative |
| `CONTRIBUTING.md` | Normative |
| `docs/decision-log.md` | Normative |
| `docs/agent-index.json` | Normative |
| `docs/open-questions.md` | Draft |
| `rubrics/QUALITY.md` | Normative |
| `rubrics/DOCUMENTATION.md` | Normative |
| `rubrics/ALIGNMENT.md` | Normative |
| `rubrics/README.md` | Normative |
| `rubrics/AGENT-READABILITY.md` | Normative |
| `rubrics/EVALUATION-PRINCIPLES.md` | Normative |
| `prompts/assumption-audit.md` | Draft |
| `prompts/api-contract-review.md` | Draft |
| `prompts/steelman-challenge.md` | Draft |
| `llms.txt` | Normative |
| `plans/llm-friendliness-remediation.md` | Archived |

## Repository Layout

```
README.md                  Project overview, quickstart, docs index
AGENTS.md                  This file (LLM agent onboarding/orientation)
CONTRIBUTING.md            Contribution workflow, full module map, invariants, testing requirements
llms.txt                   Compact retrieval manifest for agent loading
LICENSE

spec/                      Protocol specifications (authoritative)
  SPEC.md                  Architecture + lifecycle (QUIC, identity, discovery, transport)
  MESSAGE_TYPES.md         Message kinds (4) + stream mapping
  WIRE_FORMAT.md           Normative interoperable wire format
  IPC.md                   IPC protocol, Unix socket commands

docs/                      Operational documentation
  agent-index.json         Machine-readable subsystem map (task routing, file discovery)
  decision-log.md          Architectural decisions with rationale
  open-questions.md        Unresolved ambiguities

rubrics/                   Evaluation rubrics (quality, documentation, alignment, agent-readability)

prompts/                   Structured review and interrogation protocols
  assumption-audit.md      Pre-commitment plan interrogation
  api-contract-review.md   IPC/wire-format change review checklist
  steelman-challenge.md    Adversarial plan review with structured verdicts

axon/                      Rust implementation (Cargo crate)
  Cargo.toml               Dependencies and package metadata (Rust 2024 edition)
  Makefile                 Canonical build/test/verify entrypoints
  src/
    main.rs                CLI entrypoint (thin delegator to app::run)
    lib.rs                 Crate root
    app/                   Binary-only code (CLI, doctor, examples)
      mod.rs               App module declarations
      run.rs               CLI struct, Commands enum, run() logic, helpers
      run_tests.rs         Tests for CLI parsing and helpers
      examples.rs          Annotated example interactions
      cli/                 CLI helpers (IPC client, formatting, config commands)
        mod.rs, config_cmd.rs, format.rs, identity_output.rs, ipc_client.rs, notify_payload.rs (+ test files)
      doctor/              Doctor diagnostics and checks
        mod.rs             DoctorArgs, DoctorReport, run()
        identity_check.rs
        checks/            Split check modules (state_root, daemon_artifacts, peer_store, config)
    config/                Local YAML settings (name, port, advertise_addr)
      mod.rs, tests.rs
    daemon/                Daemon orchestration and lifecycle
    discovery/             Bonjour/mDNS candidate observations
      mod.rs, tests.rs
    identity/              Ed25519 identity + agent_id derivation
      mod.rs, tests.rs
    ipc/                   Unix socket IPC protocol + server
      mod.rs, auth.rs, protocol.rs, server.rs, client_handler.rs, server_tests.rs
    message/               Known/unknown MessageKind, Envelope, encode/decode
    peer_directory/        Logical peer authority, observations, atomic store, pin snapshots
      mod.rs, state.rs, types.rs, store.rs, tests.rs
    peer_token/            Peer token encoding/decoding
      mod.rs, tests.rs
    manifest/              Capability manifests: describe schema, bounds, remote cache
      mod.rs, types.rs, cache.rs, tests
    request_broker/        IPC handler lease and inbound request correlation
      mod.rs, tests.rs
    transport/             ConnectionManager, QUIC/TLS, generations, reconnect, framing
  tests/                   Integration, spec compliance, adversarial, e2e tests
  benches/                 Criterion benchmarks
  fuzz/                    cargo-fuzz harness + fuzz_targets/
  proptest-regressions/    Persisted proptest failures (commit these)
```

## Key Architecture

```
Client (OpenClaw/CLI) ←→ [Unix Socket IPC] ←→ AXON Daemon ←→ [QUIC/UDP] ←→ AXON Daemon ←→ [Unix Socket IPC] ←→ Client
```

- **Identity**: Ed25519 signing keypair. Agent ID is `ed25519.` plus the first 16 bytes of SHA-256 of the public key. Self-signed X.509 cert generated on each startup for QUIC TLS.
- **Discovery**: Bonjour/mDNS (`_axon._udp.local.`) broadcasts Agent ID and public key on the local link. Observations create untrusted candidates only; WAN rendezvous is out of scope.
- **Peer authority**: `PeerDirectory` is the only logical owner of enrolled identities, configured locators, live observations, the atomic `peers.json` store, and derived pin/query views.
- **Transport**: `ConnectionManager` exclusively owns QUIC handles, one generation-checked slot per peer, cross-dial selection, tracked tasks, and reconnect backoff.
- **IPC**: Unix socket at `~/.axon/axon.sock`; line-delimited JSON commands are `send`, `peers`, `status`, `whoami`, `add_peer`, `remove_peer`, `who_can`, `serve`, and `reply`. `RequestBroker` correlates an inbound request with exactly one terminal reply on its original QUIC stream.
- **Doctor CLI**: `axon doctor` checks state-root health, identity material, local config, canonical peer-store integrity, and unsupported legacy state.
- **Messages**: JSON envelopes with UUID, kind, payload, and optional ref. The five interpreted kinds are `request`, `response`, `message`, `error`, and `describe`; unknown strings are retained losslessly. `describe` is answered by the receiving daemon from the manifest published at `serve` time (see `axon/src/manifest/`).

## Module Map (summary)

Use this to navigate quickly; for the full "change → file(s)" table, see `CONTRIBUTING.md`.

- **Daemon lifecycle / orchestration**: `axon/src/daemon/`
- **Discovery observations (Bonjour/mDNS)**: `axon/src/discovery/`
- **Transport (QUIC/TLS/connections/framing)**: `axon/src/transport/`
- **Message kinds + envelopes + encode/decode**: `axon/src/message/`
- **IPC protocol + server**: `axon/src/ipc/`
- **IPC client handler**: `axon/src/ipc/client_handler.rs`
- **Identity + agent_id derivation**: `axon/src/identity/`
- **Config parsing**: `axon/src/config/`
- **Peer authority + pinning**: `axon/src/peer_directory/`
- **Inbound request correlation**: `axon/src/request_broker/`
- **Capability manifests**: `axon/src/manifest/`
- **CLI**: `axon/src/app/` (CLI definitions in `app/run.rs`, helpers in `app/cli/`)
- **Doctor diagnostics**: `axon/src/app/doctor/`

## Key Invariants (summary)

These are load-bearing. Do not change behavior without updating spec + tests. Full list: `CONTRIBUTING.md`.

- **Configuration reference**: when adding or changing a configurable setting (in `Config` / `config.yaml`) or an internal constant (timeout, limit, interval, etc.), update the Configuration Reference tables in `README.md`.
- **Agent ID = `ed25519.` + first 16 bytes of SHA-256(pubkey)**: peer identity must match the TLS certificate/public key; reject mismatches.
- **Intentional peer pinning**: discovery never authorizes TLS. Only explicitly enrolled peers appear in immutable pinning snapshots; revocation persists before transport authority is removed.
- **Ownership**: `PeerDirectory` owns logical peer state, `ConnectionManager` owns physical connection state, and `RequestBroker` owns request correlation. Do not introduce a parallel mutable representation.
- **Locator conflicts**: conflicting observations are quarantined and excluded from dialing; a trusted identity is never evicted because an address was reused.
- **Connection generations**: each peer has one authoritative slot; losers are closed, and stale attempt/teardown outcomes cannot mutate a newer generation.
- **Institutional memory**: when making an architectural decision, record it in `docs/decision-log.md`. When encountering an ambiguity that cannot be resolved from existing normative documents, log it in `docs/open-questions.md`.

## Building & Verification

The `Makefile` in `axon/` is canonical. Run commands from `axon/`.

```sh
cd axon
make check        # fast typecheck
make fmt          # rustfmt
make lint         # clippy -D warnings
make test-unit    # quick unit tests
make test-all     # full test suite
make verify       # fmt + lint + test-all (pre-commit default)
```

Optional (requires additional tooling):

```sh
make coverage         # cargo llvm-cov (summary)
make coverage-html    # HTML report
make fuzz             # cargo-fuzz (nightly)
make mutants-fast     # cargo-mutants focused subset
make mutants          # broader mutation testing (slower)
```

## Testing Conventions

Detailed requirements and recipes live in `CONTRIBUTING.md`. Key conventions:

- **Unit tests live inside their module directory**: directory modules use `tests.rs`, while leaf modules may use `<name>_tests.rs`; wire them from the implementation via:
  ```rust
  #[cfg(test)]
  #[path = "tests.rs"]
  mod tests;
  ```
- **Integration/spec/adversarial/e2e tests** are in `axon/tests/`:
  - `make test-integration` — integration + spec compliance + adversarial
  - `make test-e2e` — daemon lifecycle
- **Property-based tests** use `proptest`. Commit `proptest-regressions/` when generated.
- **Fuzz targets** live in `axon/fuzz/fuzz_targets/`. Add one for any new deserialization entrypoint.
- **Mutation testing** via `cargo-mutants` validates test suite quality.
- **File size limit**: all Rust source files (`.rs`) must stay under 500 lines. Split into submodules when approaching.
- **Module structure conventions**: all top-level modules are directory modules (`<name>/mod.rs`), binary-only code lives under `app/`, tests live inside their module directory. Full rules in `CONTRIBUTING.md` § "Module structure conventions".

## Nested AGENTS index

- `axon/src/app/AGENTS.md`: binary-only CLI code, doctor diagnostics, examples.
- `axon/src/config/AGENTS.md`: YAML config parsing, README co-change rules.
- `axon/src/daemon/AGENTS.md`: daemon orchestration, lifecycle, resource bounds.
- `axon/src/discovery/AGENTS.md`: Bonjour/mDNS observations and candidate-only boundary.
- `axon/src/identity/AGENTS.md`: Ed25519 identity, agent ID derivation, key format rules.
- `axon/src/ipc/AGENTS.md`: IPC protocol, server, client handler, auth, bounded queues.
- `axon/src/message/AGENTS.md`: message kinds, envelope schema, wire format compliance.
- `axon/src/manifest/AGENTS.md`: capability manifests, describe answering, remote manifest cache.
- `axon/src/peer_directory/AGENTS.md`: peer authority, atomic persistence, observations, and immutable pin views.
- `axon/src/peer_token/AGENTS.md`: peer token encoding/decoding, round-trip invariant.
- `axon/src/request_broker/AGENTS.md`: handler lease and exactly-once request correlation.
- `axon/src/transport/AGENTS.md`: connection ownership, QUIC/TLS, generations, reconnect, framing.

Maintenance rule: when adding, removing, or renaming major subsystem directories, update this index and the affected nested `AGENTS.md` files in the same change.

## Prompt Templates

Reusable structured review and interrogation protocols in `prompts/`:

| Prompt | Purpose |
|---|---|
| `prompts/assumption-audit.md` | Pre-commitment interrogation — surface hidden assumptions before implementing a plan |
| `prompts/api-contract-review.md` | Contract review checklist for IPC, wire format, and message type changes |
| `prompts/steelman-challenge.md` | Adversarial plan review with independent evidence gathering and structured verdicts |

## Specs to Read First

1. `spec/SPEC.md` — architecture + lifecycle (identity, discovery, transport)
2. `spec/MESSAGE_TYPES.md` — message kinds (4), stream mapping
3. `spec/WIRE_FORMAT.md` — normative interoperable wire format
4. `spec/IPC.md` — IPC protocol, Unix socket commands
5. `CONTRIBUTING.md` — contribution workflow, full module map, invariants, testing requirements
6. `docs/decision-log.md` — prior architectural decisions (search before proposing alternatives)
7. `docs/open-questions.md` — unresolved ambiguities (do not silently resolve)
8. `llms.txt` — compact retrieval manifest for agent loading
