# Run the full validation suite: fmt check, clippy, tests, wasm, js e2e, audit.
check: fmt-check lint test check-wasm test-js audit

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

# Verify the crate (including wasm-bindgen exports) builds for browsers.
check-wasm:
    cargo check --target wasm32-unknown-unknown --features wasm

# Build the web wasm package into pkg/.
build-wasm:
    wasm-pack build --target web --out-dir pkg --out-name ohttp_client -- --features wasm

# Build wasm and run the JS e2e against the Rust test harness.
test-js: build-wasm
    node js/e2e.test.js

# Audit dependencies for known security advisories.
audit:
    cargo audit
