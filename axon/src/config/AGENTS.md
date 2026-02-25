# AGENTS.md (config)

This file applies to configuration parsing code in `axon/src/config/`.

## Priorities

Correctness > usability > extensibility.

## File responsibilities

- `mod.rs`: `Config` struct, YAML deserialization, static peer parsing, hostname resolution.

## Guardrails

- When adding or changing any config key, update `README.md` Configuration Reference tables in the same change.
- Hostname peers are resolved at load time (IPv4 preferred); unresolvable peers are skipped with warning logs.
- Config file is optional — all settings have sensible defaults.

## Test targets

- Unit: `tests.rs`
- CLI contract: `axon/tests/cli_contract_config.rs`
