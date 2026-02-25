# AXON — Agent eXchange Over Network

Status: Normative

LLM-first local messaging protocol + Rust daemon/CLI for secure agent-to-agent communication over QUIC.

## Status

Working implementation. The Rust crate in `axon/` includes the daemon, CLI, IPC, QUIC transport, mDNS discovery, static peer config, and a full test/fuzz/bench harness. Specs in `spec/` are authoritative; if implementation disagrees, the spec wins.

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
| `rubrics/QUALITY.md` | Normative |
| `rubrics/DOCUMENTATION.md` | Normative |
| `rubrics/ALIGNMENT.md` | Normative |
| `rubrics/AGENT-READABILITY.md` | Normative |
| `rubrics/EVALUATION-PRINCIPLES.md` | Normative |

## Repository Layout

```
README.md                  Project overview, quickstart, docs index
AGENTS.md                  This file (LLM agent onboarding/orientation)
CONTRIBUTING.md            Contribution workflow, full module map, invariants, testing requirements
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
        checks/            Split check modules (state_root, daemon_artifacts, known_peers, config)
    config/                YAML config parsing (name, port, peers)
      mod.rs, tests.rs
    daemon/                Daemon orchestration, lifecycle, reconnect
    discovery/             mDNS + static peer discovery
      mod.rs, tests.rs
    identity/              Ed25519 identity + agent_id derivation
      mod.rs, tests.rs
    ipc/                   Unix socket IPC protocol + server
      mod.rs, auth.rs, protocol.rs, server.rs, client_handler.rs, server_tests.rs
    message/               MessageKind (4 variants), Envelope, encode/decode
    peer_table/            Peer storage, pinning, shared PubkeyMap
      mod.rs, tests/ (basic.rs, eviction.rs, proptest.rs)
    peer_token/            Peer token encoding/decoding
      mod.rs, tests.rs
    transport/             QUIC/TLS, connections, framing
  tests/                   Integration, spec compliance, adversarial, e2e tests
  benches/                 Criterion benchmarks
  fuzz/                    cargo-fuzz harness + fuzz_targets/
  proptest-regressions/    Persisted proptest failures (commit these)
```

## Key Architecture

```
Client (OpenClaw/CLI) ←→ [Unix Socket IPC] ←→ AXON Daemon ←→ [QUIC/UDP] ←→ AXON Daemon ←→ [Unix Socket IPC] ←→ Client
```

- **Identity**: Ed25519 signing keypair. Agent ID derived from SHA-256 of public key. Self-signed X.509 cert generated on each startup for QUIC TLS.
- **Discovery**: mDNS (`_axon._udp.local.`) broadcasts agent ID and public key. Static peers via config file for Tailscale/VPN. Plain async functions.
- **Transport**: QUIC via `quinn`. TLS 1.3 with forward secrecy. Unidirectional streams for fire-and-forget messages, bidirectional streams for request/response.
- **IPC**: Unix domain socket at `~/.axon/axon.sock`. Line-delimited JSON. 5 commands: `send`, `peers`, `status`, `whoami`, `add_peer`. Inbound messages are broadcast to connected clients; lagging clients are disconnected when bounded IPC queues overflow.
- **Doctor CLI**: `axon doctor` runs local diagnostics and optional repairs for state-root health, identity material, config hygiene, and peer-cache hygiene (including duplicate-address detection).
- **Messages**: JSON envelopes with UUID, kind, payload, and optional ref. 4 kinds: `request`, `response`, `message`, `error`.

## Module Map (summary)

Use this to navigate quickly; for the full "change → file(s)" table, see `CONTRIBUTING.md`.

- **Daemon lifecycle / reconnection**: `axon/src/daemon/`
- **Discovery (mDNS + static peers)**: `axon/src/discovery/`
- **Transport (QUIC/TLS/connections/framing)**: `axon/src/transport/`
- **Message kinds + envelopes + encode/decode**: `axon/src/message/`
- **IPC protocol + server**: `axon/src/ipc/`
- **IPC client handler**: `axon/src/ipc/client_handler.rs`
- **Identity + agent_id derivation**: `axon/src/identity/`
- **Config parsing**: `axon/src/config/`
- **Peer table + pinning**: `axon/src/peer_table/`
- **CLI**: `axon/src/app/` (CLI definitions in `app/run.rs`, helpers in `app/cli/`)
- **Doctor diagnostics**: `axon/src/app/doctor/`

## Key Invariants (summary)

These are load-bearing. Do not change behavior without updating spec + tests. Full list: `CONTRIBUTING.md`.

- **Configuration reference**: when adding or changing a configurable setting (in `Config` / `config.yaml`) or an internal constant (timeout, limit, interval, etc.), update the Configuration Reference tables in `README.md`.
- **Agent ID = SHA-256(pubkey)**: peer identity must match TLS certificate/public key; reject mismatches.
- **Peer pinning**: unknown peers must not be accepted at TLS/transport; peers must be in the PeerTable's shared PubkeyMap before connection.
- **Address uniqueness**: at most one non-static peer per network address; stale entries are evicted when a new identity appears at the same address.
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

- **Unit tests live in sibling `*_tests.rs` files**, wired from the module via:
  ```rust
  #[cfg(test)]
  #[path = "foo_tests.rs"]
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
- `axon/src/daemon/AGENTS.md`: daemon orchestration, lifecycle, reconnect, resource bounds.
- `axon/src/discovery/AGENTS.md`: mDNS/DNS-SD, static peer fallback, PeerTable integration.
- `axon/src/identity/AGENTS.md`: Ed25519 identity, agent ID derivation, key format rules.
- `axon/src/ipc/AGENTS.md`: IPC protocol, server, client handler, auth, bounded queues.
- `axon/src/message/AGENTS.md`: message kinds, envelope schema, wire format compliance.
- `axon/src/peer_table/AGENTS.md`: peer storage, pinning, PubkeyMap, address uniqueness.
- `axon/src/peer_token/AGENTS.md`: peer token encoding/decoding, round-trip invariant.
- `axon/src/transport/AGENTS.md`: QUIC/TLS, connections, framing, security invariants.

Maintenance rule: when adding, removing, or renaming major subsystem directories, update this index and the affected nested `AGENTS.md` files in the same change.

## Specs to Read First

1. `spec/SPEC.md` — architecture + lifecycle (identity, discovery, transport)
2. `spec/MESSAGE_TYPES.md` — message kinds (4), stream mapping
3. `spec/WIRE_FORMAT.md` — normative interoperable wire format
4. `spec/IPC.md` — IPC protocol, Unix socket commands
5. `CONTRIBUTING.md` — contribution workflow, full module map, invariants, testing requirements
6. `docs/decision-log.md` — prior architectural decisions (search before proposing alternatives)
7. `docs/open-questions.md` — unresolved ambiguities (do not silently resolve)
