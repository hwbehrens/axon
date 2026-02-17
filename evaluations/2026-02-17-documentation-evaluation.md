# AXON Documentation Evaluation (Rubric: `rubrics/DOCUMENTATION.md`)

Date: 2026-02-17  
Scope: Current working tree in `/Users/hwb/Projects/axon` (including uncommitted changes)

## Method

- Reviewed `README.md`, `AGENTS.md`, `CONTRIBUTING.md`, and all files in `spec/`.
- Cross-checked documentation claims against implementation in `axon/src/`.
- Validated CLI help output with:
  - `cd axon && cargo run -- --help`
  - `cd axon && cargo run -- daemon --help`
  - `cd axon && cargo run -- send --help`

## Findings and Deductions

### 1) Spec Accuracy & Interop Documentation (`-8`)

- `-3` SPEC claims all non-daemon CLI commands use IPC, but `identity` reads local files directly.
  - Doc claim: `spec/SPEC.md:201`
  - Implementation: `axon/src/main.rs:108`, `axon/src/main.rs:110`
- `-3` IPC spec says `peers` lists connected peers only; implementation returns full table with multiple statuses/sources.
  - Doc statement: `spec/IPC.md:53`
  - Implementation returns all peers: `axon/src/daemon/command_handler.rs:113`, `axon/src/daemon/command_handler.rs:121`, `axon/src/daemon/command_handler.rs:123`
- `-2` SPEC dependency block is stale vs actual crate versions.
  - Spec block: `spec/SPEC.md:278`, `spec/SPEC.md:279`
  - Actual crate: `axon/Cargo.toml:14`, `axon/Cargo.toml:15`

### 2) README & Configuration Reference (`-0`)

- README quickstart, command list, and constants table are materially aligned with current defaults and code constants (`README.md:149`, `README.md:177`, `README.md:181`, `README.md:187`).
- Config surface documented (`README.md:157`) matches `Config` (`axon/src/config.rs:73`).

### 3) Agent/Contributor Guidance (`-0`)

- `AGENTS.md` and `CONTRIBUTING.md` module maps and invariant guidance map correctly to the current crate structure (`AGENTS.md:61`, `CONTRIBUTING.md:21`, `CONTRIBUTING.md:45`).

### 4) Code-Level Documentation & Self-Documenting Code (`-1`)

- `-1` Minor clarity gap: duplicated cryptographic derivation logic appears in two modules without shared helper/reference, increasing drift risk for future updates.
  - `axon/src/identity.rs:140`
  - `axon/src/transport/tls.rs:281`

### 5) Examples, CLI Help, and Learnability (`-1`)

- `-1` Learnability gap for multi-agent-per-host workflows: docs emphasize alternate AXON roots (`spec/SPEC.md:177`, `spec/SPEC.md:180`), but non-daemon CLI commands do not expose an `--axon-root` selector.
  - Daemon has `--axon-root` (`axon/src/main.rs:37`).
  - `send`/`notify`/`peers`/`status` command definitions do not expose root selection (`axon/src/main.rs:41`, `axon/src/main.rs:43`, `axon/src/main.rs:45`, `axon/src/main.rs:47`).

### 6) Change Communication & Reviewability (`-3`)

- `-3` Repo lacks a central change log/release notes artifact explaining spec-impacting deltas; reviewers must infer historical behavior from diffs and spec edits.
  - No `CHANGELOG.md` present at repo root; version-impacting statements are distributed across multiple docs (`README.md`, `spec/*.md`, workflow checks).

## Score Sheet

| Category | Max | Score |
|---|---:|---:|
| 1) Spec Accuracy & Interop Documentation | 30 | 22 |
| 2) README & Configuration Reference | 20 | 20 |
| 3) Agent/Contributor Guidance | 15 | 15 |
| 4) Code-Level Documentation & Self-Documenting Code | 15 | 14 |
| 5) Examples, CLI Help, Learnability | 10 | 9 |
| 6) Change Communication & Reviewability | 10 | 7 |
| **Total** | **100** | **87** |

## Summary

Documentation coverage is broad and mostly accurate, especially README and contributor guidance. Primary deductions come from spec/implementation drift in a few normative statements and from missing centralized change communication.
