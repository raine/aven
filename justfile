# Rust project checks

set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# List available commands
default:
    @just --list

# Run the fast local read-only check set
check: check-fast-readonly migration-order clippy

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

# Run every check, including tests and redundant compile gates
check-full: check test pre-merge

# Configure Git to use the repository hooks
install-hooks:
    git config core.hooksPath hooks
    scripts/install-git-hook-shims

# Install local tools used by quality gates
install-quality-tools:
    cargo install checkle cargo-deny cargo-machete cargo-nextest sqlx-cli

# Provision the persistent CuaBot verification image
cua-sandbox-setup:
    scripts/cua-sandbox setup

# Snapshot a host database into an isolated CuaBot session
cua-sandbox-db session *ARGS:
    scripts/cua-sandbox db-bootstrap {{session}} {{ARGS}}

# Start an isolated CuaBot verification session
cua-sandbox-start session:
    scripts/cua-sandbox start {{session}}

# Stop an isolated CuaBot verification session
cua-sandbox-stop session:
    scripts/cua-sandbox stop {{session}}

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

# Build and verify the frozen iOS Rust artifact matrix
ios-rust-artifacts:
    scripts/build-ios-rust-artifacts

# Package and verify the frozen iOS SwiftPM artifact matrix
ios-swiftpm-package: ios-rust-artifacts
    scripts/package-ios-swiftpm

# Build, test, install, and launch the minimal iOS host proof
ios-host-proof: ios-swiftpm-package
    ios/run-host-proof all

# Build the macOS Rust facade and generate Swift bindings
uniffi-swift:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(git rev-parse --show-toplevel)"
    host="$(rustc -vV | while IFS= read -r line; do
      case "$line" in
        "host: "*) printf '%s\n' "${line#host: }" ;;
      esac
    done)"
    if [[ -z "$host" ]]; then
      echo "Error: rustc did not report a host target" >&2
      exit 1
    fi
    target_dir="$root/target/aven-uniffi/build"
    output="$root/target/aven-uniffi/swift"
    package_generated="$root/experiments/aven-swift-cli/Generated"
    rm -rf "$output" "$package_generated"
    mkdir -p \
      "$output" \
      "$package_generated/lib" \
      "$package_generated/Sources/AvenUniFFI" \
      "$package_generated/Sources/aven_uniffiFFI"
    CARGO_TARGET_DIR="$target_dir" MACOSX_DEPLOYMENT_TARGET=13.0 \
      cargo build --release --locked -p aven-uniffi --target "$host"
    CARGO_TARGET_DIR="$target_dir" MACOSX_DEPLOYMENT_TARGET=13.0 \
      cargo run --release --locked -p aven-uniffi --bin uniffi-bindgen \
      --target "$host" -- generate \
      --library "$target_dir/$host/release/libaven_uniffi.dylib" \
      --language swift \
      --out-dir "$output"
    cp "$target_dir/$host/release/libaven_uniffi.a" \
      "$package_generated/lib/libaven_uniffi.a"
    cp "$output/aven_uniffi.swift" \
      "$package_generated/Sources/AvenUniFFI/aven_uniffi.swift"
    cp "$output/aven_uniffiFFI.h" \
      "$package_generated/Sources/aven_uniffiFFI/aven_uniffiFFI.h"
    cp "$output/aven_uniffiFFI.modulemap" \
      "$package_generated/Sources/aven_uniffiFFI/module.modulemap"

# Build, test, and run the macOS Swift local and sync proofs
swift-local-proof: uniffi-swift
    cargo build --locked --bin aven
    swift build --package-path experiments/aven-swift-cli
    swift test --package-path experiments/aven-swift-cli
    swift run --package-path experiments/aven-swift-cli aven-local-proof local
    swift run --package-path experiments/aven-swift-cli aven-local-proof sync "$(pwd)/target/debug/aven"

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
    db="$(pwd)/target/sqlx-prepare.sqlite"
    rm -f "$db"
    DATABASE_URL="sqlite://$db" cargo sqlx database create
    DATABASE_URL="sqlite://$db" cargo sqlx migrate run --source crates/aven-core/migrations
    (cd crates/aven-core && DATABASE_URL="sqlite://$db" cargo sqlx prepare -- --all-targets)

# Check sqlx offline query metadata
sqlx-check:
    #!/usr/bin/env bash
    set -euo pipefail
    db="$(pwd)/target/sqlx-check.sqlite"
    rm -f "$db"
    scripts/quiet-check sqlx-create env DATABASE_URL="sqlite://$db" cargo sqlx database create
    scripts/quiet-check sqlx-migrate env DATABASE_URL="sqlite://$db" cargo sqlx migrate run --source crates/aven-core/migrations
    (cd crates/aven-core && ../../scripts/quiet-check sqlx-check env DATABASE_URL="sqlite://$db" cargo sqlx prepare --check -- --all-targets --locked)

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
      crates/aven-core/migrations
      crates/aven-core/.sqlx
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

# Install release binary globally from local source and restart the local daemon when sync is enabled
install:
    env SQLX_OFFLINE=true cargo install --offline --path . --locked
    if [[ "$("$HOME/.cargo/bin/aven" config get sync.enabled)" == "true" ]]; then "$HOME/.cargo/bin/aven" daemon install; fi

# Install release binary globally from GitHub releases
install-release:
    #!/usr/bin/env bash
    set -euo pipefail
    install_root="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}"
    AVEN_INSTALL_DIR="$install_root/bin" bash scripts/install

# Install debug binary globally when the daemon is not installed
install-dev:
    #!/usr/bin/env bash
    set -euo pipefail
    daemon_plist="$HOME/Library/LaunchAgents/com.raine.aven.daemon.plist"
    if [[ -f "$daemon_plist" ]]; then
        echo "Error: install-dev cannot replace the binary used by the installed daemon" >&2
        echo "Use 'just install' for a daemon-safe local installation, or uninstall the daemon first" >&2
        exit 1
    fi
    cargo build
    ln -sf "$(pwd)/target/debug/aven" "$HOME/.cargo/bin/aven"

# Run the application against the dev database when configured
run *ARGS:
    env ${AVEN_DEV_DB:+AVEN_DB="$AVEN_DEV_DB"} cargo run -- "$@"

# Run the TUI against the dev database when configured
tui:
    env ${AVEN_DEV_DB:+AVEN_DB="$AVEN_DEV_DB"} cargo run -- tui

# Run the docs development server
docs:
    #!/usr/bin/env bash
    set -euo pipefail
    bun install --cwd docs --frozen-lockfile
    preferred_port=4321
    max_port=$((preferred_port + 50))
    for ((port = preferred_port; port <= max_port; port++)); do
        if ! nc -z 127.0.0.1 "$port" >/dev/null 2>&1; then
            exec bun run --cwd docs dev --port "$port"
        fi
    done
    echo "Error: could not find an available docs port between ${preferred_port} and ${max_port}" >&2
    exit 1

# Internal release helper
_release bump:
    @cargo-release {{bump}}

# Release a new patch version
release *ARGS:
    @just _release patch {{ARGS}}
