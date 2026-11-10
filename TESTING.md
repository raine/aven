# Testing

Use focused checks while iterating, then hand off each affected package. Run the
full gate once on the integrated tree; this routing is a starting heuristic, not
automatic diff-based selection.

## Focused iteration

Focused tests use the full suite's `SQLX_OFFLINE=true` and `target/test`
artifacts:

```sh
just test-lib aven-core sync::
just test-target aven cli_local
```

`test-lib` takes a libtest/nextest test-name filter. Filters match substrings, so
end a module filter with `::` (for example, `sync::`) to avoid matching unrelated
names containing the same text. `test-target` runs the named integration-test
target; target names use Rust/Cargo naming, such as `cli_local` for
`tests/cli_local.rs`.

## Test quality

A useful test adds independent information. Ask whether a plausible wrong
implementation would fail it and a correct refactor would pass. Prefer fewer,
stronger tests; extend an existing test when that keeps related coverage clear.
For a bug fix, verify the regression test fails without the fix when feasible.

Treat these as review signals, not deletion rules:

- Assertions that only restate constants, defaults, or mappings; tests of pure
derived or standard-library behavior; expected values built with the same helper
under test.
- Echo mocks, existence-only checks, incidental structure assertions, and
coverage duplicated across layers.

Retain independently meaningful behavior, including custom codec behavior,
observable API, CLI, and serialized contracts, compatibility fixtures,
invariants, boundary and failure cases, and cross-module behavior.

Tests run on Rust's default 2 MiB test-thread stack. Test builds compile
workspace crates at opt-level 0. At that level, an async function's poll frame
reserves a separate slot for every future it awaits, so dispatchers with many
arms and test bodies with many awaits use far more stack than release builds.
When a test aborts with `has overflowed its stack`, find the large future, for
example with `RUSTC_BOOTSTRAP=1 cargo rustc --profile test --lib -p <package>
-- -Zprint-type-sizes` in a separate target directory. Box that future where it
is defined, or box the call that every caller awaits, rather than raising
`RUST_MIN_STACK`.

Ordinary database fixtures copy a blank template instead of migrating a new
file per test. `aven_core::test_support::open_blank_database` builds one fully
migrated, checkpointed, single-file database per test binary build under
`aven-db-templates/` next to the binary, copies it to the test's private path,
and opens the copy through `Database::open`. The template omits
`meta.client_id`, so every copy receives its own installation identity. Use
`Database::open` on a new path instead when the test's subject is first-open
behavior: migrations, historical schemas, WAL handling, backups, restores, or
installation identity.

## Container packaging

Build the checked-out source and exercise the real container on your host's
architecture (`linux/amd64` or `linux/arm64`):

```sh
docker build -t aven:local .
scripts/test-container-image aven:local linux/arm64
AVEN_IMAGE=aven:local docker compose config --quiet
shellcheck scripts/test-container-image
actionlint .github/workflows/container.yml .github/workflows/release.yml
```

The smoke script uses disposable containers and a named volume. It checks
non-root setup, refusal of unprepared storage, the HTTP router, graceful
SIGTERM, restart, and replacement-container persistence. Setup invitations are
captured privately and deleted; do not upload raw setup output as test logs.
Container CI builds source images on native amd64 and arm64 runners.

The release workflow checksums the existing Linux musl archives, builds the
Dockerfile's `release` target with the `release-bin` named build context, and
runs both architectures (arm64 under QEMU). That context contains
`linux-amd64/aven` and `linux-arm64/aven`. Saved, tested images are passed to
publication without rebuilding. Hyphenated tags verify images without
publishing. Stable tags publish exact versions and promote `latest` only after
both platform pulls pass and GitHub identifies the tag as its latest release.

Registry publication needs separate verification: make the GHCR package
public, confirm its repository link and both platform descriptors, and pull the
version anonymously on **both** platforms. Use a temporary empty Docker config
rather than changing your normal registry credentials:

```sh
config_dir=$(mktemp -d)
docker --config "$config_dir" pull --platform linux/amd64 ghcr.io/raine/aven:VERSION
docker --config "$config_dir" pull --platform linux/arm64 ghcr.io/raine/aven:VERSION
rm -rf "$config_dir"
```

Replace `VERSION` with the published version. If Docker uses a custom context,
set `DOCKER_HOST` to its daemon endpoint first; an empty configuration does not
include saved contexts. Local builds and authenticated
workflow pulls do not establish public visibility or anonymous access. Release
tags and registry publishing are remote mutations; only run them when
authorized.

## macOS release packaging

Each macOS architecture (`arm64`, `amd64`) is built twice and published under
separate names per channel. Updaters choose by channel and never fall back to
the other channel's files (`ReleaseChannel` in `src/update/release.rs`).

| Channel | Asset | Used by |
| --- | --- | --- |
| Terminal | `aven-darwin-<arch>.tar.gz` | `aven update` from a bare binary, Nix |
| Terminal | `aven-darwin-<arch>-app.tar.gz` | `aven update`, `scripts/install`, the formula |
| Desktop | `Aven-<arch>.dmg` | People downloading Desktop, the cask |
| Desktop | `aven-desktop-darwin-<arch>-app.tar.gz` | The Desktop updater |

Terminal builds omit the `desktop` feature. Each asset has a checksum file
named after it with `.sha256` in place of its extension, for example
`Aven-arm64.sha256`. Disk image names carry no version, so
`releases/latest/download/Aven-arm64.dmg` always points at the newest
release. Released updaters refuse terminal archives over 64 MiB, so the release
checks that limit for them.

Apps and bare binaries are signed as `fi.zendit.Aven`; disk images use their
default signing identifier. All use the same Developer ID. Apps and disk images
are notarized with `scripts/notarize-macos-release` and stapled. The release fails unless
`spctl --assess` reports each shipped app and disk image as notarized. The bare
terminal binary is notarized too but cannot hold a ticket; Gatekeeper looks its
ticket up online. Notarization uses an App Store Connect API key from the
`macos-signing` environment: `ASC_KEY_ID`, `ASC_ISSUER_ID`, and `ASC_KEY_P8`,
which holds the `.p8` file contents.

The disk image window layout lives in `macos/dmg`: `background.svg`, the
`background.tiff` rendered from it at 1x and 2x, and the `DS_Store` that sets
the window size, icon positions and background. `scripts/create-macos-dmg`
copies them as they are, so release builds need neither Finder nor Python.
After changing the background or the layout, run
`scripts/generate-macos-dmg-layout` (needs `rsvg-convert` and `uv`) and commit
its outputs. Finder finds the background by the volume name `Aven`, so another
mounted volume named `Aven` hides it.

Stable tags publish the assets and push `Formula/aven.rb` and `Casks/aven.rb`
to `raine/homebrew-aven`. Hyphenated tags such as `v1.2.3-signtest.1` build,
sign and notarize without publishing or updating the tap.

Local fixtures need no credentials:

```sh
scripts/test-homebrew-formula
scripts/test-homebrew-cask
scripts/test-assemble-macos-app
scripts/test-create-macos-dmg
scripts/test-package-macos-binary
```

### Desktop updates

The Desktop updater accepts archives up to 192 MiB. It downloads only when
asked, waits for every other process running the app's executable except the
daemon, and never stops them: an older version that is already running keeps
writing to a database the new version migrated, while a fresh older process
refuses to open it. The previous bundle stays in `.aven-update-previous` beside
`Aven.app` until the new version confirms its launch, and comes back only if
the database schema is unchanged.

Debug builds can run the whole flow against fixtures, apart from the installed
app and real data:

- `AVEN_DEV_RELEASE_API` replaces the GitHub latest-release URL, and allows
  `http://127.0.0.1` asset URLs.
- `AVEN_DEV_APPLICATIONS` stands in for `/Applications`, so an app assembled
  there with `scripts/assemble-macos-app` skips Setup.
- Isolate `HOME`, the XDG directories and `AVEN_DB` as `scripts/demo-app`
  does, give the bundle its own identifier, and open it with `open --env`.

Only signed releases show that a notarized bundle passes Gatekeeper after the
swap and that launchd restarts the daemon on the new executable.

## Affected-package handoff

`just test-package <package>` runs that package's non-documentation test targets.
`just check-package <package>` is the handoff check: all-target Clippy followed
by those package tests.

```sh
just test-package aven-core
just check-package aven
```

To opt in to a report of passing tests slower than one second, use
`just profile-tests` with the same package, target, or test-name filters as
`just _test`, for example `just profile-tests --package aven --lib
peer_enrollment_http::tests::`. The `slow-tests` nextest profile reports slow
statuses without changing normal test or `check-full` output.

The default nextest profile uses 12 test threads. Test builds use debug level 1,
which retains source-line tables for backtraces while reducing debugger detail
such as local-variable information. To compare suite concurrency, warm
`target/test` and repeat the full suite at 6, 8, and 12 threads:

```sh
env SQLX_OFFLINE=true cargo nextest run \
    --target-dir target/test --locked --workspace --all-targets \
    --no-fail-fast --status-level fail --test-threads 12
```

To compare test-profile builds, use separate empty target directories with
`cargo nextest run --no-run --locked --workspace --all-targets`, setting
`CARGO_PROFILE_TEST_DEBUG=2` or `1` in each environment.

The test profile keeps workspace crates at opt-level 0 and builds dependencies
at opt-level 2, except the proc macros and build tools named under
`[profile.dev.package]`, which it inherits at opt-level 0. This speeds warm
full-suite runs at the cost of slower cold no-run builds. Debug level 1 retains line tables, but optimized dependency code
can inline frames and omit locals, reducing stepping and backtrace detail.

To rebenchmark, compare this profile with the package override removed, using
separate clean target directories for cold builds and alternating warm full
suites at 12 threads. Record wall, user, and system time for both phases. Keep
sccache disabled or hold its cache state equal; on macOS and Linux,
`RUSTC_WRAPPER=/usr/bin/env` bypasses the configured wrapper.

Select the package that owns the changed behavior; workspace members are listed
in the root `Cargo.toml`. The CLI/TUI (`aven`) and Desktop (`aven-desktop`) share
`aven-host`, which depends on `aven-core`. If a core or host API or contract
change affects a consumer, check both packages. Include `--features desktop`
when checking the root app's Desktop integration. Database migration or SQLx
query/metadata changes also need `just sqlx-check`.

The pre-commit hook runs format, dependency/static-safety, and migration-order
checks; it does not infer affected packages from staged paths. For coordinated
worktree changes, each agent runs `just check-package` for its affected package
before handoff. The coordinator/integrator relies on the existing pre-push hook
to run `just check-full` once, after integrating the changes. Do not repeat the
full gate in every worktree.

## Full gate on the integrated tree

`just check-full` preserves the repository-wide gate: `just check`, the full
test suite, and deferred SQLx validation. All-target nextest compilation also
emits the normal `aven` binary used by integration tests, so no separate
workspace build is needed. Use the full gate once after integration. If
the affected package or boundary is unclear, changes span packages, workspace
configuration or shared test infrastructure changes, or focused checks leave
material risk, choose the conservative full gate rather than guessing.

| Changed area | Initial route |
| --- | --- |
| Root app code outside the rows below (`src/`, root `tests/`) | `just check-package aven` |
| TUI (`src/tui/`) | `just check-package aven` |
| CLI and commands (`src/cli/`, `src/commands/`, `tests/cli_*.rs`) | `just check-package aven` |
| Host logic (`crates/aven-host/src/`: configuration, routing, operations, the store, sync host) | `just check-package aven-host`; include `just check-package aven` for CLI or TUI behavior that depends on it |
| Core (`crates/aven-core/src/`, `crates/aven-core/tests/`) | `just check-package aven-core` |
| Sync (`crates/aven-core/src/sync/`, app/server sync paths) | `just check-package aven-core`; include `just check-package aven` for affected app/server paths or consumer contracts |
| Database (`crates/aven-core/src/db/`, migrations, SQLx metadata) | `just check-package aven-core`; also `just sqlx-check` for migrations or SQLx inputs |
| Website (`docs/`) | `cd docs && bun install --frozen-lockfile && bun run build` |
| Cross-package, workspace, or uncertain changes | Run the integrated-tree `just check-full` |

If a change spans rows or does not fit a route, treat it as cross-package or
uncertain and use the integrated full gate.
