# AGENTS.md (config)

This file applies to configuration parsing code in `axon/src/config/`.

## Priorities

Correctness > usability > extensibility.

## File responsibilities

- `mod.rs`: local daemon settings, path discovery, YAML deserialization, and legacy peer-state rejection.

## Guardrails

- When adding or changing any config key, update `README.md` Configuration Reference tables in the same change.
- Config file is optional — all settings have sensible defaults.
- Peer trust and locators do not belong in `config.yaml`; they are owned by `PeerDirectory` and persisted in `peers.json`.
- Reject legacy `peers:` entries and `known_peers.json` rather than silently importing them.

## Test targets

- Unit: `tests.rs`
- CLI contract: `axon/tests/cli_contract_config.rs`
