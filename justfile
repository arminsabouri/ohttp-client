# Run the full validation suite: fmt check, clippy, tests, audit.
check: fmt-check lint test audit

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

# Audit dependencies for known security advisories.
audit:
    cargo audit
