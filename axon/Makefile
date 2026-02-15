.PHONY: all test test-unit test-integration test-e2e test-all fmt lint check fuzz fuzz-target coverage coverage-html coverage-lcov mutants mutants-fast clean verify ci

# Default: format, lint, and run all tests
all: fmt lint test-all

# Quick unit tests only (~1s)
test-unit:
	cargo test --lib

# Integration + spec + adversarial tests (~0.5s)
test-integration:
	cargo test --test integration --test spec_compliance --test adversarial

# End-to-end daemon lifecycle tests (~10s)
test-e2e:
	cargo test --test daemon_lifecycle

# All tests (unit + integration + e2e)
test-all:
	cargo test

# Alias
test: test-all

# Format code
fmt:
	cargo fmt

# Lint (must be warning-free)
lint:
	cargo clippy -- -D warnings

# Type check only (fast)
check:
	cargo check

# Run all fuzz targets for 30 seconds each
fuzz:
	@echo "=== Fuzzing all targets (30s each) ==="
	cargo +nightly fuzz run fuzz_envelope_decode -- -max_total_time=30
	cargo +nightly fuzz run fuzz_wire_decode -- -max_total_time=30
	cargo +nightly fuzz run fuzz_ipc_command -- -max_total_time=30
	cargo +nightly fuzz run fuzz_config_toml -- -max_total_time=30
	cargo +nightly fuzz run fuzz_known_peers -- -max_total_time=30
	cargo +nightly fuzz run fuzz_envelope_roundtrip -- -max_total_time=30
	@echo "=== All fuzz targets passed ==="

# Run a specific fuzz target: make fuzz-target TARGET=fuzz_envelope_decode [DURATION=60]
DURATION ?= 30
fuzz-target:
	cargo +nightly fuzz run $(TARGET) -- -max_total_time=$(DURATION)

# Code coverage summary (text)
coverage:
	cargo llvm-cov --summary-only

# Code coverage with HTML report
coverage-html:
	cargo llvm-cov --html --open

# Code coverage with lcov output (for CI)
coverage-lcov:
	cargo llvm-cov --lcov --output-path lcov.info

# Mutation testing (library code, excluding main.rs CLI)
mutants:
	cargo mutants --timeout 120 --exclude-re 'main\.rs' -- --lib

# Mutation testing (fast subset: message + config + peer_table)
mutants-fast:
	cargo mutants --timeout 60 --file 'src/message/**' --file 'src/config.rs' --file 'src/peer_table.rs' -- --lib

# Clean build artifacts
clean:
	cargo clean
	rm -rf mutants.out lcov.info

# Pre-commit: the full verification suite
verify: fmt lint test-all
	@echo "✅ All checks passed"

# CI: everything including coverage
ci: fmt lint test-all coverage
	@echo "✅ CI pipeline passed"
