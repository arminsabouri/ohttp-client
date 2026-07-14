# Run the full validation suite: fmt check, clippy, tests, wasm, audit.
check: fmt-check lint test check-wasm audit

# Format all code.
fmt:
    cargo fmt --all

# Check formatting without writing changes.
fmt-check:
    cargo fmt --all -- --check

# Lint with clippy, treating warnings as errors, across all features.
lint:
    cargo clippy --all-targets --all-features -- -D warnings

clippy: lint

# Run tests across all features.
test:
    cargo test --all-features

# Verify the sans-IO crate builds for browsers with the JS RNG backend.
check-wasm:
    cargo check --target wasm32-unknown-unknown --features wasm_js

# Audit dependencies for known security advisories.
audit:
    cargo audit
