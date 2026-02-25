# AGENTS.md (app)

This file applies to binary-only CLI code in `axon/src/app/`.

## Priorities

Usability > correctness > consistency with spec.

## File responsibilities

- `run.rs`: CLI struct (`Cli`), `Commands` enum, `run()` entrypoint, argument parsing.
- `examples.rs`: Annotated example interactions for `axon examples`.
- `mod.rs`: App module declarations.
- `cli/`: CLI helpers — `ipc_client.rs` (daemon communication), `format.rs` (output formatting), `config_cmd.rs` (config get/set/list), `identity_output.rs`, `notify_payload.rs`.
- `doctor/`: Doctor diagnostics — `mod.rs` (report runner), `identity_check.rs`, `checks/` (split check modules).

## Guardrails

- Binary-only boundary: never import `app::` from library modules (`lib.rs` tree).
- CLI command changes → update `axon/tests/cli_contract.rs`.
- Doctor behavior changes → update `axon/tests/doctor_contract.rs`.
- Help text and examples changes → update `README.md`.
- Exit codes: 0 (success), 1 (local/runtime failure), 2 (usage/application failure), 3 (request timeout).

## Test targets

- Unit: `run_tests.rs`, `cli/config_cmd_tests.rs`, `cli/format_tests.rs`, `cli/ipc_client_tests.rs`, `cli/notify_payload_tests.rs`
- CLI contract: `axon/tests/cli_contract.rs`, `axon/tests/cli_contract_config.rs`
- Doctor contract: `axon/tests/doctor_contract.rs`
- Spec compliance: `axon/tests/spec_compliance/cli_help.rs`
