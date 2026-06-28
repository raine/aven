# Architecture

`aven` is a local-first task manager implemented as one Rust crate and binary. It provides a CLI, Ratatui TUI, SQLite persistence, HTTP sync server, and local daemon wake path. This file is a sitemap for coding agents. For operator-facing command usage, also read `docs/agent-usage.txt` and `src/skill.md`.

## System map

| Layer | Owns | Start here | Rules |
| --- | --- | --- | --- |
| CLI entry and dispatch | argument parsing, command routing, config load, database open, daemon wake | `src/main.rs`, `src/lib.rs`, `src/cli.rs`, `src/commands.rs`, `src/commands/` | Command-family modules own command orchestration and command-local formatting. Business writes belong in operations or mutation helpers. |
| Write model | transactional task, project, label, conflict, config, and workspace changes | `src/operations/`, `src/mutation.rs`, `src/task_fields.rs` | Synced scalar task writes must update tasks, `changes`, and `field_versions` together. |
| Read model | task lists, project lists, sidebar counts, filters, sorting, refs, and enrichment | `src/query.rs`, `src/query/`, `src/task_enrichment.rs`, `src/refs.rs`, `src/queue.rs` | Batch task-list enrichment. Avoid per-row queries on list paths. |
| Persistence | SQLite setup, migrations, sync metadata, conflict helpers, SQLx metadata | `src/db.rs`, `migrations/`, `.sqlx/` | Create migrations with `just migration-new <lower_snake_name>`. Refresh SQLx metadata after query or schema changes. |
| Config and routing | config files, managed config text edits, path mappings, workspace resolution, project inference | `src/config.rs`, `src/config_edit.rs`, `src/workspaces.rs`, `src/projects.rs` | `src/config.rs` owns durable config text writes. Managed entry text surgery belongs in `src/config_edit.rs`. Workspace-scoped commands must resolve an active workspace before domain lookup. |
| Sync and daemon | HTTP sync client/server, wire DTOs, remote apply, wake loop | `src/sync.rs`, `src/sync/`, `src/sync/apply/`, `src/daemon.rs` | Wire shapes, protocol version, server validation, and remote apply semantics must evolve together. |
| TUI app | event loop, actions, overlays, store, rendering, undo, natural add runtime, platform helpers | `src/tui/` | Store modules own DB access. UI modules render view models. Overlay routes drive behavior, not titles. Natural add worker setup belongs in `src/tui/natural_add_runtime.rs`. |
| Shared domain helpers | IDs, choices, labels, input loading, text rendering, CLI render output, logging, fuzzy matching | `src/ids.rs`, `src/choices.rs`, `src/labels.rs`, `src/input.rs`, `src/render.rs`, `src/task_render.rs`, `src/logging.rs`, `src/fuzzy.rs`, `src/types.rs` | Reuse canonical helpers instead of duplicating validation, display, diff, or parsing rules. |
| Tests and tooling | CLI integration tests, TUI/store tests, overlay module tests, SQL index checks, just tasks | `tests/`, `src/tui/*tests.rs`, `src/tui/overlay/`, `justfile` | Add focused tests near the subsystem and rely on commit hooks for the full gate. |

## Runtime flows

### CLI command flow

1. `src/main.rs` starts Tokio and calls `aven::run_cli()`.
2. `src/cli.rs` parses `Cli` and `Commands`.
3. `src/lib.rs` initializes logging, handles commands that do not need the task database, resolves config and database path, opens SQLite, resolves workspace scope when needed, then dispatches to `src/commands.rs` and focused command-family modules under `src/commands/`.
4. Mutating commands call operations or mutation helpers and wake the daemon when sync is enabled.

### TUI flow

1. `src/tui/mod.rs` initializes Ratatui and constructs `App`.
2. Input resolves through the command catalog in `src/tui/event/` unless a capturing overlay handles it first.
3. `App` holds TUI flow state. Focused `src/tui/app_*.rs` modules coordinate feature flows, then call `TuiStore` facade methods in `src/tui/store.rs`.
4. `TaskViewState` is the source of truth for TUI task lists. It carries scope, view, filter modifiers, flat order, and direction, then derives query filters, query mode, and render mode.
5. Store modules call the same operations, mutation, and query helpers as the CLI.
6. Natural-language add flow coordination lives in `src/tui/app_authoring.rs`; background worker command construction, environment propagation, process setup, and log path selection live in `src/tui/natural_add_runtime.rs`.
7. `src/tui/ui.rs` and `src/tui/ui/` render state. Rendering code should not touch the database.

### Sync flow

1. Local mutations append operation-log rows in `changes`.
2. Unsynced rows have `server_seq IS NULL`.
3. `src/sync/client.rs` posts at most `MAX_PUSH_BATCH` unsynced rows, the current `sync_cursor` as `after`, and `MAX_PULL_BATCH` as `pull_limit` to `/sync`.
4. `src/sync/wire.rs` owns the protocol version, request limits, cursor validation, response validation, and daemon sync budget constants.
5. `src/sync/server.rs` validates operation names, entity types, protocol version, batch bounds, pull limits, cursors, and payload shapes before assigning server sequence numbers.
6. The server assigns strictly increasing `server_seq` values to accepted changes, reuses existing values for duplicate pushed change IDs, returns one bounded pull page, and sets `has_more` when rows remain after the page.
7. Remote apply updates local tables transactionally, records conflicts for scalar field version mismatches, applies push acknowledgements, then advances `sync_cursor` to the response cursor.
8. `src/sync/client.rs` validates each response before applying it. Response changes must fit the requested pull limit, have strictly increasing `server_seq` values after the cursor, match the response cursor, and include one push acknowledgement for each pushed change.
9. The CLI sync path drains bounded push and pull pages until local unsynced backlog and remote `has_more` are both empty.
10. `src/daemon.rs` calls `run_sync_with_page_budget` with `DAEMON_SYNC_PAGE_BUDGET`; incomplete daemon rounds report counts, cursor, completion, and page count, then schedule prompt follow-up sync work.

## Data ownership

SQLite stores synced task data and local UI state. Config files store local routing and service settings.

- Synced domain tables: `workspaces`, `tasks`, `projects`, `labels`, `task_labels`, `notes`, `task_dependencies`.
- Sync bookkeeping: `changes`, `field_versions`, `conflicts`, `meta`.
- Local-only config: database path, sync settings, project path mappings, directory overrides.
- Local-only TUI state: view, filter, selection, overlay, sort state, and `tui_undo_entries`; pending undo entries are cleared when a TUI store starts.
- Backup and portability workflows: `src/commands/data_safety.rs` for snapshot/restore flows and `src/db.rs` for backup file naming.

`Task` and `Project` in `src/types.rs` are core records. Workspace-scoped tables include `workspace_id` in uniqueness and lookup paths. Many invariants are application-enforced rather than database-enforced, so do not write domain tables directly unless a change is intentionally bypassing sync and validation.

## Domain rules

- Status values are `inbox`, `backlog`, `todo`, `active`, `done`, and `canceled`.
- Priority values are `none`, `low`, `medium`, `high`, and `urgent`.
- New entity IDs come from `crate::ids::new_id()`, which returns 16 Crockford Base32 characters from 80 random bits.
- Timestamps come from `crate::ids::now()` and are UTC strings.
- Project IDs are stable identity. Project keys and names are lookup and display fields.
- Project renames update key, name, and prefix on the same stable project ID.
- Projects normalize names into keys with lowercase words joined by `-`.
- Project prefixes are generated to be unique and are display context, not task identity.
- Labels normalize before storage and must exist before assignment.
- Task refs resolve by ID suffix, optionally qualified as `PREFIX-SUFFIX`.
- Typed task ref suffixes must be at least 3 characters. Display refs use at least 4 suffix characters and lengthen to disambiguate current tasks.
- `O` normalizes to `0`, and `I` or `L` normalize to `1` when resolving refs.

## Architectural guardrails

- Use `src/operations/` or `src/mutation.rs` for writes that affect synced domain data.
- Use `src/query.rs`, `src/query/`, and enrichment helpers for read models.
- Keep scalar task fields aligned across validation, task rows, `changes`, `field_versions`, sync apply, and conflict resolution.
- Keep workspace scope explicit on queries and mutations that operate on user data.
- Keep config serialization and durable text writes in `src/config.rs`; keep managed-entry text transforms in `src/config_edit.rs`.
- Keep CLI output formatting in command or render modules, not in persistence helpers. Use `src/render.rs` for shared quoting, changed flag text, multiline blocks, near-match errors, and text diffs.
- Keep TUI database access in `src/tui/store/`; keep `src/tui/ui/` rendering-only.
- Derive TUI task list filters, query mode, and render mode from `TaskViewState`; do not keep parallel project, status, view, or queue-sort state.
- Treat project selection in the TUI as scope. Project scope must not be modeled as a filter modifier or view.
- TUI overlays carry `OverlayRoute`; behavior resolves through `OverlayRoute::descriptor` and never depends on title text. Titles are render-only chrome.
- TUI shortcuts use intent prefixes in the command catalog. Navigation and scope use `g`, named views use `v`, composable filters use `f`, ordering uses `o`, selected-task actions use `t`, project administration uses `p`, label administration uses `L`, conflicts use `c`, and config uses `C`.
- Overlay dialogs should use shared helpers in `src/tui/ui/dialog.rs` for title edges, frame clearing, background, border, and footer hint styling.
- Overlay behavior tests live in the overlay module they exercise under `src/tui/overlay/`; the facade in `src/tui/overlay.rs` stays focused on module wiring and exports.
- Record a TUI undo entry for completed TUI mutations unless the action is undo itself; pending TUI undo entries are valid only within the current `TuiStore` lifecycle and are cleared on store startup.
- Do not log auth tokens, raw sync payloads, task descriptions, note bodies, user-authored labels or project names, or secret config values.
- Keep sync protocol changes aligned across `src/sync/wire.rs`, `src/sync/client.rs`, `src/sync/server.rs`, `src/sync/apply.rs`, and `src/daemon.rs`. Request and response validation must match the client page loop and server page construction.
- Keep bounded sync limits explicit: `MAX_PUSH_BATCH` bounds client push pages, `MAX_PULL_BATCH` bounds server pull pages, and `DAEMON_SYNC_PAGE_BUDGET` bounds daemon work per wake.
- Keep cursor semantics based on `server_seq`. Pull pages are ordered by increasing `server_seq`; response cursors equal the last returned `server_seq` or the request cursor for an empty page; local `sync_cursor` advances only after a validated page applies successfully.
- Keep daemon sync privacy-safe and budget-aware. Daemon logs and stdout include counts, cursor, completion, and page count without user content, and incomplete rounds schedule prompt follow-up sync work.

## Change routing

| Change | Start here | Also check | Tests |
| --- | --- | --- | --- |
| Add or change a CLI command | `src/cli.rs`, `src/lib.rs`, `src/commands.rs`, `src/commands/` | `src/operations/` for writes, `src/input.rs` for text input, `src/render.rs` for shared output helpers, `src/task_render.rs` for task output | focused `tests/cli_*.rs` |
| Add a task scalar field | migration, `src/types.rs`, `src/task_fields.rs`, `src/mutation.rs` | `src/operations/tasks.rs`, `src/sync/apply/task.rs`, `src/sync/apply/conflict.rs`, `src/sync/wire.rs`, `src/query/`, CLI and TUI renderers | sync, conflict, CLI, and TUI tests |
| Add task dependency relations | `src/operations/dependencies.rs`, `src/query/dependencies.rs` | `src/commands.rs`, `src/task_render.rs`, `src/sync/apply/dependency.rs`, `src/sync/server.rs` | `tests/cli_dependencies.rs`, `tests/cli_sync.rs` |
| Change task list, filters, sorting, or refs | `src/query/`, `src/query.rs`, `src/refs.rs`, `src/queue.rs` | CLI list rendering, `src/tui/store/types.rs`, `src/tui/store/view.rs`, indexes | query unit tests, `tests/sqlite_read_path_indexes.rs`, focused CLI tests |
| Add or change a TUI action | `src/tui/event/catalog.rs`, `src/tui/app_dispatch.rs`, focused `src/tui/app_*.rs` modules | flow helpers, overlays, store module, undo, `src/tui/natural_add_runtime.rs` for natural-add worker setup | `src/tui/app_tests.rs`, `src/tui/store/tests.rs`, overlay module tests |
| Add or change TUI overlay behavior | `src/tui/overlay.rs`, `src/tui/overlay/`, `src/tui/app_overlay_submit.rs` | `OverlayRoute::descriptor`, input helpers, state builders, view projection, submit dispatch, module-local tests | `cargo test tui::overlay` |
| Add or change TUI overlay rendering | `src/tui/ui/overlays.rs`, `src/tui/ui/overlays/` | overlay view models, shared dialog helpers, input helpers, theme | overlay rendering tests in `src/tui/ui/overlays/tests.rs` |
| Change sync protocol or conflict handling | `src/sync/wire.rs`, `src/sync/apply/`, `src/sync/server.rs`, `src/sync/client.rs` | `src/mutation.rs`, `src/task_fields.rs`, `src/daemon.rs`, migrations if persisted | `tests/cli_sync*.rs`, `tests/cli_conflicts.rs`; focused bounded-sync checks include `cargo test --test cli_sync sync_server_returns_bounded_pull_pages`, `cargo test --test cli_sync sync_client_drains_paged_remote_changes`, `cargo test --test cli_sync sync_client_drains_paged_local_changes`, `cargo test --test cli_sync current_protocol_version_sync_succeeds`, and `cargo test --test cli_sync wrong_response_protocol_version_is_rejected` |
| Add or change backup, export, or import commands | `src/cli.rs`, `src/lib.rs`, `src/commands/data_safety.rs`, `src/db.rs` | doctor output and integrity checks in `src/commands.rs` | `tests/cli_data_safety.rs`, `tests/cli_doctor.rs` |
| Change config, workspace, or project path routing | `src/config.rs`, `src/config_edit.rs`, `src/workspaces.rs`, `src/projects.rs` | config text writes, managed-entry edits, doctor, project commands, TUI workspace and project pickers | `tests/cli_config_daemon.rs`, `tests/cli_workspaces.rs`, `tests/cli_doctor.rs` |
| Change natural-language task intake or agent primer | `src/task_intake.rs`, `src/skill.md` | config schema, `aven prime`, add-task flows, `src/tui/natural_add_runtime.rs` for TUI background worker setup | `tests/cli_task_intake.rs`, focused add-task tests |
| Change logging | `src/logging.rs` and call sites | safe field policy in guardrails | `tests/cli_logging.rs` |

## Common feature checklists

### Add a CLI command

1. Add args and a `Commands` variant in `src/cli.rs`.
2. Add dispatch, workspace needs, and daemon wake behavior in `src/lib.rs`.
3. Add command handling and output formatting in `src/commands.rs` or a focused command module under `src/commands/`.
4. Put transactional business logic in `src/operations/`.
5. Add integration tests in `tests/`.

### Add a task scalar field

1. Create a migration with `just migration-new <lower_snake_name>`.
2. Update `Task` in `src/types.rs` and row mapping in refs or query code.
3. Update create payloads, update DTOs, operations, and mutation validation.
4. Seed field versions during task creation if the field needs conflict protection.
5. Update sync wire/apply behavior and conflict resolution.
6. Update CLI rendering, TUI rendering, filters, or sorting if exposed there.
7. Run `just sqlx-prepare` after query or migration changes.

### Add a TUI overlay route

1. Add the `OverlayRoute` variant and include it in `OverlayRoute::ALL`.
2. Add one arm to `OverlayRoute::descriptor` for submit kind, picker mode, and fallback message.
3. Add the submit route enum variant that matches the input kind.
4. Add the corresponding `handle_*_submit` branch in `src/tui/app_overlay_submit.rs`.
5. Branch on `route` in `src/tui/ui/overlays/` when rendering differs by route.
6. Add focused overlay or app overlay tests for input, submit, and rendering behavior.

### Change bounded sync behavior

1. Update the wire contract and shared constants in `src/sync/wire.rs`.
2. Keep the client loop in `src/sync/client.rs` aligned with push batch limits, pull limits, response validation, and cursor advancement.
3. Keep the server handler in `src/sync/server.rs` aligned with request validation, sequence allocation, duplicate push acknowledgements, and bounded pull pages.
4. Keep remote apply in `src/sync/apply/` transaction-safe and cursor-safe.
5. Keep daemon work in `src/daemon.rs` bounded by `DAEMON_SYNC_PAGE_BUDGET`, with incomplete rounds scheduling follow-up sync.
6. Use focused validation commands: `cargo test --test cli_sync sync_server_returns_bounded_pull_pages`, `cargo test --test cli_sync sync_client_drains_paged_remote_changes`, `cargo test --test cli_sync sync_client_drains_paged_local_changes`, `cargo test --test cli_sync current_protocol_version_sync_succeeds`, `cargo test --test cli_sync wrong_response_protocol_version_is_rejected`, and `cargo test --test cli_daemon_sync daemon_syncs_large_backlog_across_budgeted_rounds`.

### Add a TUI action

1. Add an `Action` variant and register it in the command catalog under `src/tui/event/`.
2. Route action execution through `App` dispatch helpers.
3. Add or reuse overlay state and set the correct `OverlayRoute` for submitted input.
4. Add flow helpers when the action spans multiple submits.
5. Add `TuiStore` facade methods and focused store logic.
6. Record undo for mutating actions.
7. Add tests for shortcut resolution, action dispatch, overlay route propagation, and store behavior.

## Development and validation

Use `just` as the main development entrypoint:

- `just check`: local read-only validation gate, equivalent to `just pre-commit`.
- `just test`: Rust test suite through `cargo nextest`, plus Rust doctests.
- `just migration-new <lower_snake_name>`: create the next SQLx migration filename safely.
- `just sqlx-prepare`: regenerate SQLx offline query metadata after migrations or query shape changes.
- `just sqlx-check`: verify SQLx offline query metadata.
- `just run -- ...`: run the application.

The pre-commit hook runs formatting, static analysis, migration order checks, clippy, tests, and doctests. Local project instructions say cargo format and broad tests run automatically on commit, so run focused commands while developing and let the hook run the full gate when committing.
