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
pre-commit: check-fast-readonly migration-order

# Run SQLx validation deferred until workmux merge
pre-merge: sqlx-check-if-needed

# Run every check, including tests and deferred SQLx validation
check-full: check test pre-merge

# Configure Git to use the repository hooks
install-hooks:
    git config core.hooksPath hooks
    scripts/install-git-hook-shims

# Install local tools used by quality gates
install-quality-tools:
    cargo install checkle cargo-machete cargo-nextest sqlx-cli

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
    @scripts/quiet-check clippy-fix cargo clippy --workspace --fix --allow-dirty --target-dir target/clippy --all-targets -- -D warnings -W clippy::all

# Build the project
build:
    @scripts/quiet-check build cargo build --all --locked

# Type-check all targets without producing final artifacts
check-types:
    @scripts/quiet-check check-types cargo check --workspace --all-targets --locked

# Compile the committed Aven.app icon resources from macos/Aven.icon (needs Xcode 26)
macos-icon:
    #!/usr/bin/env bash
    set -euo pipefail
    out=$(mktemp -d)
    trap 'rm -rf "$out"' EXIT
    xcrun actool macos/Aven.icon --compile "$out" --platform macosx \
        --minimum-deployment-target 11.0 --app-icon Aven \
        --output-partial-info-plist "$out/partial.plist" >/dev/null
    for key in CFBundleIconFile CFBundleIconName; do
        expected=$(plutil -extract "$key" raw "$out/partial.plist")
        actual=$(plutil -extract "$key" raw macos/Info.plist)
        if [[ $actual != "$expected" ]]; then
            echo "macos/Info.plist $key is '$actual'; actool expects '$expected'" >&2
            exit 1
        fi
    done
    cp "$out/Assets.car" "$out/Aven.icns" macos/Resources/

# Test generic locking, hook isolation and worktree setup
public-tooling-test:
    scripts/test-process-lock
    scripts/test-pre-commit
    scripts/test-sqlx-check-if-needed
    scripts/test-workmux-environment

# Run tests
test:
    @checkle run tests

# Run tests with the same environment and target directory as the full suite
_test *ARGS:
    env SQLX_OFFLINE=true cargo nextest run --target-dir target/test --locked --no-fail-fast --status-level fail {{ARGS}}

# Run library tests matching a test-name filter
test-lib package filter:
    just _test --package {{package}} --lib {{filter}}

# Run one integration-test target
test-target package target:
    just _test --package {{package}} --test {{target}}

# Run all non-documentation test targets in one package
test-package package:
    just _test --package {{package}} --all-targets

# Run a fuzz target on stable Rust; `just fuzz --list` names them
fuzz target *ARGS:
    scripts/fuzz {{target}} {{ARGS}}

# Report passing tests that take longer than one second
profile-tests *ARGS:
    env SQLX_OFFLINE=true cargo nextest run --target-dir target/test --locked --no-fail-fast --profile slow-tests {{ARGS}}

# Lint every target in one package
_clippy-package package:
    @scripts/quiet-check "clippy {{package}}" cargo clippy --message-format=json --target-dir target/clippy --package {{package}} --all-targets -- -D warnings -D clippy::all

# Run the package handoff checks
check-package package:
    just _clippy-package {{package}}
    just test-package {{package}}

# Generate sqlx offline query metadata
sqlx-prepare:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target
    db="$(pwd)/target/sqlx-prepare.sqlite"
    rm -f "$db"
    DATABASE_URL="sqlite://$db" cargo sqlx database create
    DATABASE_URL="sqlite://$db" cargo sqlx migrate run --source crates/aven-core/migrations
    (cd crates/aven-core && DATABASE_URL="sqlite://$db" cargo sqlx prepare -- --lib)

# Check sqlx offline query metadata
sqlx-check:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target
    db="$(pwd)/target/sqlx-check.sqlite"
    rm -f "$db"
    scripts/quiet-check sqlx-create env DATABASE_URL="sqlite://$db" cargo sqlx database create
    scripts/quiet-check sqlx-migrate env DATABASE_URL="sqlite://$db" cargo sqlx migrate run --source crates/aven-core/migrations
    (cd crates/aven-core && ../../scripts/quiet-check sqlx-check env DATABASE_URL="sqlite://$db" cargo sqlx prepare --check -- --lib --locked)

# Check sqlx offline query metadata when SQLx inputs changed
sqlx-check-if-needed:
    @scripts/sqlx-check-if-needed

# Run installed static analysis tools
static-analysis:
    @checkle run static-analysis

# Install release binary globally from local source and restart the local daemon when sync is enabled
# On macOS, installs Aven.app so notifications and Login Items show Aven
install:
    #!/usr/bin/env bash
    set -euo pipefail
    bin="$HOME/.cargo/bin/aven"
    if [[ "$(uname -s)" == "Darwin" ]]; then
        env SQLX_OFFLINE=true cargo build --release --offline --locked
        pkgid=$(cargo pkgid --offline)
        version=${pkgid##*[#@]}
        app_dir="$HOME/.local/share/aven"
        staging=$(mktemp -d "${TMPDIR:-/tmp}/aven-app.XXXXXX")
        trap 'rm -rf "$staging"' EXIT
        scripts/assemble-macos-app target/release/aven "$version" "$staging" >/dev/null
        mkdir -p "$app_dir" "$(dirname "$bin")"
        # Swap the whole bundle so a running daemon keeps its old executable.
        rm -rf "$app_dir/.Aven.app.old"
        if [[ -d "$app_dir/Aven.app" ]]; then mv "$app_dir/Aven.app" "$app_dir/.Aven.app.old"; fi
        mv "$staging/Aven.app" "$app_dir/Aven.app"
        rm -rf "$app_dir/.Aven.app.old"
        ln -sfn "$app_dir/Aven.app/Contents/MacOS/aven" "$bin.tmp.$$"
        mv -f "$bin.tmp.$$" "$bin"
        echo "Installed $app_dir/Aven.app ($version) and linked $bin"
    else
        env SQLX_OFFLINE=true cargo install --offline --path . --locked
    fi
    if [[ "$("$bin" config get sync.enabled)" == "true" ]]; then "$bin" daemon install; fi

# Install release binary globally from GitHub Actions
install-ci:
    scripts/install-ci

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
