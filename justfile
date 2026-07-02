# Rust project checks

set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# List available commands
default:
    @just --list

# Run the local read-only check set
check: check-fast-readonly migration-order clippy test

# Run cheap read-only checks in parallel
check-fast-readonly:
    @checkle run fast-readonly

# Check migration filenames and branch ordering
migration-order:
    @scripts/quiet-check migration-order scripts/check-migration-order

# Create a SQLx migration with the next safe timestamp
migration-new name:
    @scripts/new-migration {{name}}

# Run commit-time checks without mutating files
pre-commit: check

# Run checks that are deferred until workmux merge
pre-merge: sqlx-check-if-needed build

# Run every check, including redundant compile gates
check-full: check pre-merge

# Configure Git to use the repository hooks
install-hooks:
    git config core.hooksPath hooks
    scripts/install-git-hook-shims

# Install local tools used by quality gates
install-quality-tools:
    cargo install checkle cargo-deny cargo-machete cargo-nextest sqlx-cli

# Run the full gate and fail if there are uncommitted changes
check-ci: check-full
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
    @checkle run format-check

# Run clippy and fail on any warnings
clippy:
    @checkle run clippy

# Auto-fix clippy warnings
clippy-fix:
    @scripts/quiet-check clippy-fix cargo clippy --fix --allow-dirty --target-dir target/clippy --all-targets -- -D warnings -W clippy::all

# Build the project
build:
    @scripts/quiet-check build cargo build --all --locked

# Type-check all targets without producing final artifacts
check-types:
    @scripts/quiet-check check-types cargo check --all-targets --locked

# Run tests
test:
    @checkle run tests

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
    scripts/quiet-check sqlx-check env DATABASE_URL="sqlite://$db" cargo sqlx prepare --check -- --all-targets --locked

# Check sqlx offline query metadata when SQLx inputs changed
sqlx-check-if-needed:
    #!/usr/bin/env bash
    set -euo pipefail
    target="${WM_TARGET_BRANCH:-main}"
    if ! git rev-parse --verify --quiet "$target^{commit}" >/dev/null; then
      echo "run sqlx-check: target ref '$target' not found"
      just sqlx-check
      exit 0
    fi
    mapfile -t merge_bases < <(git merge-base --all HEAD "$target" 2>/dev/null || true)
    if [[ "${#merge_bases[@]}" -ne 1 ]]; then
      echo "run sqlx-check: expected one merge base with '$target', got ${#merge_bases[@]}"
      just sqlx-check
      exit 0
    fi
    sqlx_paths=(
      Cargo.lock
      Cargo.toml
      ':(glob)**/Cargo.toml'
      build.rs
      ':(glob)**/build.rs'
      migrations
      .sqlx
      ':(glob)**/*.rs'
    )
    if git diff --quiet "${merge_bases[0]}" HEAD -- "${sqlx_paths[@]}"; then
      echo "skip sqlx-check: SQLx inputs unchanged against $target"
      exit 0
    fi
    just sqlx-check

# Run installed static analysis tools
static-analysis:
    @checkle run static-analysis

# Install release binary globally and restart the local daemon
install:
    env SQLX_OFFLINE=true cargo install --offline --path . --locked
    "$HOME/.cargo/bin/aven" daemon install

# Install debug binary globally via symlink
install-dev:
    cargo build && ln -sf $(pwd)/target/debug/aven ~/.cargo/bin/aven

# Run the application against the dev database when configured
run *ARGS:
    env ${AVEN_DEV_DB:+AVEN_DB="$AVEN_DEV_DB"} cargo run -- "$@"

# Run the TUI against the dev database when configured
tui:
    env ${AVEN_DEV_DB:+AVEN_DB="$AVEN_DEV_DB"} cargo run -- tui

# Internal release helper
_release bump:
    @cargo-release {{bump}}

# Release a new patch version
release *ARGS:
    @just _release patch {{ARGS}}
