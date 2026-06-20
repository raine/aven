# Rust project checks

set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# List available commands
default:
    @just --list

# Run all checks
check: clippy-fix pre-commit

# Run cheap read-only checks in parallel
[parallel]
check-fast-readonly: format-check static-analysis

# Run commit-time checks without mutating files
pre-commit: check-fast-readonly clippy test sqlx-check

# Run every check, including redundant compile gates
check-full: check check-types build

# Configure Git to use the repository hooks
install-hooks:
    git config core.hooksPath hooks

# Install local tools used by quality gates
install-quality-tools:
    cargo install cargo-deny cargo-machete

# Run check and fail if there are uncommitted changes (for CI)
check-ci: check
    #!/usr/bin/env bash
    set -euo pipefail
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "Error: check caused uncommitted changes"
        echo "Run 'just check' locally and commit the results"
        git diff --stat
        exit 1
    fi

# Format Rust files
format:
    @scripts/quiet-check format cargo fmt --all

# Check Rust formatting without changing files
format-check:
    @scripts/quiet-check format-check cargo fmt --all -- --check

# Run clippy and fail on any warnings
clippy:
    @scripts/quiet-check clippy cargo clippy --all-targets -- -D warnings -D clippy::all

# Auto-fix clippy warnings
clippy-fix:
    @scripts/quiet-check clippy-fix cargo clippy --fix --allow-dirty --all-targets -- -D warnings -W clippy::all

# Build the project
build:
    @scripts/quiet-check build env RUSTFLAGS="-D warnings" cargo build --all --locked

# Type-check all targets without producing final artifacts
check-types:
    @scripts/quiet-check check-types env RUSTFLAGS="-D warnings" cargo check --all-targets --locked

# Run tests
test:
    @scripts/quiet-check test env RUSTFLAGS="-D warnings" cargo test --all --locked

# Generate sqlx offline query metadata
sqlx-prepare:
    #!/usr/bin/env bash
    set -euo pipefail
    db="target/sqlx-prepare.sqlite"
    rm -f "$db"
    DATABASE_URL="sqlite://$db" cargo sqlx database create
    DATABASE_URL="sqlite://$db" cargo sqlx migrate run
    DATABASE_URL="sqlite://$db" cargo sqlx prepare -- --all-targets

# Check sqlx offline query metadata
sqlx-check:
    #!/usr/bin/env bash
    set -euo pipefail
    db="target/sqlx-check.sqlite"
    rm -f "$db"
    scripts/quiet-check sqlx-create env DATABASE_URL="sqlite://$db" cargo sqlx database create
    scripts/quiet-check sqlx-migrate env DATABASE_URL="sqlite://$db" cargo sqlx migrate run
    scripts/quiet-check sqlx-check env DATABASE_URL="sqlite://$db" cargo sqlx prepare --check -- --all-targets

# Run installed static analysis tools
static-analysis:
    #!/usr/bin/env bash
    set -euo pipefail
    ran=0
    if command -v cargo-deny >/dev/null 2>&1; then
      scripts/quiet-check cargo-deny cargo deny check
      ran=1
    else
      echo "skip cargo-deny: not installed"
    fi
    if command -v cargo-machete >/dev/null 2>&1; then
      scripts/quiet-check cargo-machete cargo machete --with-metadata
      ran=1
    else
      echo "skip cargo-machete: not installed"
    fi
    if [[ "$ran" -eq 0 ]]; then
      echo "no optional static analysis tools ran; install with 'just install-quality-tools'"
    fi

# Install release binary globally
install:
    env SQLX_OFFLINE=true cargo install --offline --path . --locked

# Install debug binary globally via symlink
install-dev:
    cargo build && ln -sf $(pwd)/target/debug/atm ~/.cargo/bin/atm

# Run the application
run *ARGS:
    cargo run -- "$@"

# Internal release helper
_release bump:
    @cargo-release {{bump}}

# Release a new patch version
release *ARGS:
    @just _release patch {{ARGS}}
