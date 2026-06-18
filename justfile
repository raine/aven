# Rust project checks

set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# List available commands
default:
    @just --list

# Run all checks
check: format clippy-fix pre-commit

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
    cargo fmt --all

# Check Rust formatting without changing files
format-check:
    cargo fmt --all -- --check

# Run clippy and fail on any warnings
clippy:
    cargo clippy --all-targets -- -D clippy::all

# Auto-fix clippy warnings
clippy-fix:
    cargo clippy --fix --allow-dirty --all-targets -- -W clippy::all

# Build the project
build:
    cargo build --all --locked

# Type-check all targets without producing final artifacts
check-types:
    cargo check --all-targets --locked

# Run tests
test:
    cargo test --all --locked

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
    DATABASE_URL="sqlite://$db" cargo sqlx database create
    DATABASE_URL="sqlite://$db" cargo sqlx migrate run
    DATABASE_URL="sqlite://$db" cargo sqlx prepare --check -- --all-targets

# Run installed static analysis tools
static-analysis:
    #!/usr/bin/env bash
    set -euo pipefail
    ran=0
    if command -v cargo-deny >/dev/null 2>&1; then
      cargo deny check
      ran=1
    else
      echo "skip cargo-deny: not installed"
    fi
    if command -v cargo-machete >/dev/null 2>&1; then
      cargo machete --with-metadata
      ran=1
    else
      echo "skip cargo-machete: not installed"
    fi
    if [[ "$ran" -eq 0 ]]; then
      echo "no optional static analysis tools ran; install with 'just install-quality-tools'"
    fi

# Install release binary globally
install:
    cargo install --offline --path . --locked

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
