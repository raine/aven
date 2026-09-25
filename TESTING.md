# Testing

Use focused checks while iterating, then hand off each affected package. Run the
full gate once on the integrated tree; this routing is a starting heuristic, not
automatic diff-based selection.

## Focused iteration

Focused tests use the full suite's `SQLX_OFFLINE=true`,
`RUST_MIN_STACK=4194304`, and `target/test` artifacts:

```sh
just test-lib aven-core sync::
just test-target aven cli_local
```

`test-lib` takes a libtest/nextest test-name filter. Filters match substrings, so
end a module filter with `::` (for example, `sync::`) to avoid matching unrelated
names containing the same text. `test-target` runs the named integration-test
target; target names use Rust/Cargo naming, such as `cli_local` for
`tests/cli_local.rs`.

`just test-tui-template` prepares a fully migrated, checkpointed blank database
once, then runs keyboard-dispatch and conflict TUI tests with a private copy per
test. The template path is under `target/test-fixtures/`; ordinary test commands
keep using real database creation. Tests for migrations, historical schemas,
WAL, backups, restores, or installation identity should use the normal path.

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

The workspace packages are `aven` and `aven-core`; `aven` depends on
`aven-core`, not the reverse. Select the package that owns the changed behavior.
If a core API or contract change affects its `aven` consumer, check both
packages. Database migration or SQLx query/metadata changes also need
`just sqlx-check`.

For coordinated worktree changes, each agent runs `just check-package` for its
affected package before handoff. The coordinator/integrator relies on the
existing pre-push hook to run `just check-full` once, after integrating the
changes. Do not repeat the full gate in every worktree.

## Full gate on the integrated tree

`just check-full` preserves the repository-wide gate: `just check`, the full
test suite, and deferred SQLx/build checks. Use it once after integration. If
the affected package or boundary is unclear, changes span packages, workspace
configuration or shared test infrastructure changes, or focused checks leave
material risk, choose the conservative full gate rather than guessing.

| Changed area | Initial route |
| --- | --- |
| Root app code outside the rows below (`src/`, root `tests/`) | `just check-package aven` |
| TUI (`src/tui/`) | `just check-package aven` |
| CLI and commands (`src/cli/`, `src/commands/`, `tests/cli_*.rs`) | `just check-package aven` |
| Configuration (`src/config/` and related CLI/TUI behavior) | `just check-package aven` |
| Core (`crates/aven-core/src/`, `crates/aven-core/tests/`) | `just check-package aven-core` |
| Sync (`crates/aven-core/src/sync/`, app/server sync paths) | `just check-package aven-core`; include `just check-package aven` for affected app/server paths or consumer contracts |
| Database (`crates/aven-core/src/db/`, migrations, SQLx metadata) | `just check-package aven-core`; also `just sqlx-check` for migrations or SQLx inputs |
| Website (`docs/`) | `cd docs && bun install --frozen-lockfile && bun run build` |
| Cross-package, workspace, or uncertain changes | Run the integrated-tree `just check-full` |

If a change spans rows or does not fit a route, treat it as cross-package or
uncertain and use the integrated full gate.
