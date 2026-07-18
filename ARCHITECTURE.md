# Architecture

`aven` is a local-first task manager implemented as a Cargo workspace. The `aven-core` crate owns domain and SQLite behavior, while the root `aven` package provides the CLI, Ratatui TUI, HTTP sync transport and server, daemon, configuration routing, and update delivery. This file is a sitemap for coding agents. For operator-facing command usage, also read `docs/agent-usage.txt` and `src/skill.md`.

## System map

| Layer | Owns | Start here | Rules |
| --- | --- | --- | --- |
| CLI entry and dispatch | argument parsing, command routing, config load, core database open, daemon wake dispatch, coding-agent skill installation | `src/main.rs`, `src/lib.rs`, `src/cli.rs`, `src/commands.rs`, `src/commands/` | `src/commands.rs` is the module and re-export facade. Command-family modules own command orchestration and command-local formatting. Domain reads and writes go through `aven_core::db::Database`. |
| Core facade | owned database handle and application-facing domain operations | `crates/aven-core/src/db.rs`, `crates/aven-core/src/operations/`, `crates/aven-core/src/query.rs` | `Database` owns the SQLx pool and complete transaction boundaries. Application code passes owned domain inputs and receives owned records or reports. |
| Write model | transactional task, project, label, conflict, workspace, undo, and remote changes | `crates/aven-core/src/operations/`, `crates/aven-core/src/mutation.rs`, `crates/aven-core/src/task_fields.rs`, `crates/aven-core/src/undo.rs` | Synced scalar task writes update tasks, `changes`, and `field_versions` together. Conflict resolution emits the canonical synced operation. |
| Read model | task lists, task details, task search, project lists, sidebar counts, filters, sorting, refs, and enrichment | `crates/aven-core/src/query.rs`, `crates/aven-core/src/query/`, `crates/aven-core/src/task_enrichment.rs`, `crates/aven-core/src/refs.rs`, `crates/aven-core/src/queue.rs` | Use the task-detail model for enriched single-task reads. Keep list and search reads batch-oriented, and avoid per-row queries on list paths. |
| Persistence | SQLite setup, embedded migrations, data safety, sync metadata, conflicts, and SQLx metadata | `crates/aven-core/src/db.rs`, `crates/aven-core/src/data_safety.rs`, `crates/aven-core/migrations/`, `.sqlx/` | Create migrations with `just migration-new <lower_snake_name>`. Core APIs own SQLx handles and transaction lifetimes. Refresh SQLx metadata after query or schema changes. |
| Config and routing | config files, managed config text edits, path mappings, workspace resolution, and project inference | `src/config.rs`, `src/config_edit.rs`, `src/workspaces.rs`, `src/projects.rs`, `src/operations/projects.rs` | Application code reads files and the current directory, then passes resolved routing inputs to the core. Managed entry text surgery belongs in `src/config_edit.rs`. |
| Sync and daemon | HTTP transport and presentation, transport-neutral protocol validation and persistence, launchd service management, wake policy and loop | `src/sync.rs`, `src/sync/`, `src/daemon.rs`, `src/daemon/`, `crates/aven-core/src/sync/` | Root sync modules own Reqwest and Axum. Core sync modules own wire DTOs, request construction, validation, acknowledgements, cursor advancement, remote apply, and conflict generation. |
| TUI app | event loop, actions, overlays, store, rendering, natural add runtime, and platform helpers | `src/tui/` | `TuiStore` owns a cloned core `Database` handle. UI modules render view models and never access SQLx or SQLite directly. |
| Update delivery | cached GitHub release discovery, semantic version comparison, install ownership classification, verified direct replacement | `src/update.rs`, `src/update/` | Background checks are fail-silent and rate-limited. CLI and TUI flows own their presentation and confirmation behavior. |
| Shared domain types | validated IDs, status and priority values, task and project records, query DTOs, sync DTOs | `crates/aven-core/src/ids.rs`, `crates/aven-core/src/choices.rs`, `crates/aven-core/src/types.rs`, `crates/aven-core/src/query/types.rs`, `crates/aven-core/src/sync/wire.rs` | Domain types are independent of CLI arguments, TUI state, config files, and UniFFI. Application-only rendering and input parsing remain under root `src/`. |
| Tests and tooling | core extraction tests, CLI integration tests, TUI tests, SQL index checks, just tasks | `crates/aven-core/tests/`, `tests/`, `src/tui/*tests.rs`, `justfile` | Add focused tests near the owning package and rely on commit hooks for the full gate. |

## Runtime flows

### CLI command flow

1. `src/main.rs` starts Tokio and calls `aven::run_cli()`.
2. `src/cli.rs` parses `Cli` and `Commands`.
3. `src/lib.rs` classifies every parsed command into the explicit standalone, database, or TUI dispatch class through an exhaustive `Commands` match. Database and TUI paths open an opaque `aven_core::db::Database`, resolve application routing, then call core-owned reads and mutations.
4. Mutating commands call core operations through `Database`, then dispatch through the daemon wake-if-enabled policy.

### TUI flow

1. `src/tui/mod.rs` initializes Ratatui and constructs `App`.
2. Input resolves through the command catalog in `src/tui/event/` unless a capturing overlay handles it first.
3. `App` holds TUI flow controllers and simple UI state. Focused `src/tui/app_*.rs` modules coordinate feature flows. `SearchController` in `src/tui/app_search.rs` owns live search preview state and worker lifecycle, `IntakeController` in `src/tui/app_intake.rs` owns natural-add mode, configuration, worker lifecycle, and pending-to-ready transitions, while `UpdateController` owns update flow resources and state.
4. `TaskViewState` is the source of truth for TUI task lists. It carries scope, view, filter modifiers, flat order, and direction, then derives query filters, query mode, and render mode. `src/tui/columns.rs` derives column grouping and navigation from the configured fixed-status partition while retaining global task indexes. `src/tui/ui/task_list/` owns tabular task-list view models and hit testing, and `src/tui/ui/columns.rs` owns shared column layout, rendering, and hit testing. Spotlight search renders live overlay results from the core task search read model and stores matched task IDs in the submitted search view. The recent actions view reads the synced change log through `crates/aven-core/src/query/recent_actions.rs` and renders through `src/tui/ui/recent_actions.rs` without modeling actions as task rows.
5. Store modules call owned methods on `aven_core::db::Database`. SQLx pools, connections, transactions, and rows stay inside `aven-core`.
6. Natural-language add flow coordination lives in `src/tui/app_authoring.rs`; `IntakeController` in `src/tui/app_intake.rs` owns intake configuration, add-task-only mode, background state, cancellation, and polling transitions; background worker command construction, environment propagation, process setup, and log path selection live in `src/tui/natural_add_runtime.rs`. The add-task overlay state owns the visible draft, six-field focus, validation, and integrated metadata, help, and discard-confirmation child modes, so child controls preserve text cursor state.
7. `src/tui/ui.rs` and `src/tui/ui/` render state. Rendering code should not touch the database.
8. TUI update coordination lives in `src/tui/app_update.rs`; `src/update/` owns release discovery, cache scheduling, install classification, verification, and direct executable replacement. Update availability renders as a non-modal header badge, while explicit checks and installs use overlays.

### Sync flow

1. Local mutations append operation-log rows in `changes`.
2. Unsynced rows have `server_seq IS NULL`.
3. `src/sync/client.rs` asks the core `Database` to prepare at most `MAX_PUSH_BATCH` unsynced rows, the current `sync_cursor` as `after`, and `MAX_PULL_BATCH` as `pull_limit`, then performs the HTTP exchange.
4. `crates/aven-core/src/sync/wire.rs` owns the protocol version, request limits, cursor validation, response validation, and daemon sync budget constants.
5. `src/sync/server.rs` owns Axum presentation and authentication, while core sync persistence validates accepted operations and constructs bounded response pages.
6. The server assigns strictly increasing `server_seq` values to accepted changes, reuses existing values for duplicate pushed change IDs, returns one bounded pull page, and sets `has_more` when rows remain after the page.
7. Remote apply updates local tables transactionally, records conflicts for scalar field version mismatches, applies push acknowledgements, then advances `sync_cursor` to the response cursor.
8. `src/sync/client.rs` validates each response before applying it. Response changes must fit the requested pull limit, have strictly increasing `server_seq` values after the cursor, match the response cursor, and include one push acknowledgement for each pushed change.
9. The CLI sync path drains bounded push and pull pages until local unsynced backlog and remote `has_more` are both empty.
10. `src/daemon.rs` calls `run_sync_with_page_budget` with `DAEMON_SYNC_PAGE_BUDGET`; incomplete daemon rounds report counts, cursor, completion, and page count, then schedule prompt follow-up sync work.

## Data ownership

SQLite stores synced task data and local UI state. Config files store local routing and service settings.

- Synced domain tables: `workspaces`, `tasks`, `projects`, `labels`, `task_labels`, `notes`, `task_dependencies`, `task_epic_links`.
- Sync bookkeeping: `changes`, `field_versions`, `conflicts`, `meta`.
- Local-only config: database path, sync settings, project path mappings, directory overrides, and TUI column grouping.
- Local-only TUI state: view, filter, selection, overlay, sort state, and `tui_undo_entries`; pending undo entries are cleared when a TUI store starts.
- Backup and portability workflows: `src/commands/data_safety/mod.rs` for presentation and `crates/aven-core/src/data_safety.rs` for scans, validation, import, integrity, and backup mechanics.

`Task` and `Project` in `crates/aven-core/src/types.rs` are core records. Workspace-scoped tables include `workspace_id` in uniqueness and lookup paths. Many invariants are application-enforced rather than database-enforced, so do not write domain tables directly unless a change is intentionally bypassing sync and validation.

## Domain rules

- Status values are `inbox`, `backlog`, `todo`, `active`, `done`, and `canceled`.
- Priority values are `none`, `low`, `medium`, `high`, and `urgent`.
- New entity IDs come from typed constructors in `aven_core::ids`, which return 16 Crockford Base32 characters from 80 random bits.
- Timestamps come from `aven_core::ids::now()` and are UTC strings.
- Workspace identity uses `WorkspaceId` from `crates/aven-core/src/ids.rs` throughout domain records, queries, and mutations. CLI, config, sync payload, and export conversions validate the 16-character Crockford Base32 representation, while core persistence binds and decodes the type as SQLite text.
- Project identity uses `ProjectId` from `crates/aven-core/src/ids.rs` throughout domain records, queries, and mutations. CLI, config, sync payload, and export conversions validate the 16-character Crockford Base32 representation, while core persistence binds and decodes the type as SQLite text.
- Task identity uses `TaskId` from `crates/aven-core/src/ids.rs` throughout domain records, queries, mutations, relationships, and undo snapshots. CLI refs, sync payloads, and exports preserve validated 16-character Crockford Base32 strings, while core persistence binds and decodes the type as SQLite text.
- `Task` represents absent `available_at` and `due_on` values with `None`. SQLite, sync payloads, exports, and JSON output encode absence as an empty string at their compatibility boundaries.
- Project keys and names are lookup and display fields.
- Project renames update key, name, and prefix on the same stable project ID.
- Projects normalize names into keys with lowercase words joined by `-`.
- Project prefixes are generated to be unique and are display context, not task identity.
- Labels normalize before storage and must exist before assignment.
- Task refs resolve by ID suffix, optionally qualified as `PREFIX-SUFFIX`.
- Typed task ref suffixes must be at least 3 characters. Display refs use at least 4 suffix characters and lengthen to disambiguate current tasks.
- `O` normalizes to `0`, and `I` or `L` normalize to `1` when resolving refs.

## Architectural guardrails

- Use owned `aven_core::db::Database` methods for persisted domain reads and writes from the application package.
- Keep SQLx pools, connections, transactions, rows, migrations, and query implementation inside `crates/aven-core`.
- Use `crates/aven-core/src/operations/` or `crates/aven-core/src/mutation.rs` for writes that affect synced domain data.
- Use `crates/aven-core/src/query.rs`, `crates/aven-core/src/query/`, and core enrichment helpers for read models.
- Keep scalar task fields aligned across validation, task rows, `changes`, `field_versions`, sync apply, and conflict resolution.
- Keep workspace scope explicit on queries and mutations that operate on user data.
- Keep config serialization and durable text writes in `src/config.rs`; keep managed-entry text transforms in `src/config_edit.rs`.
- Keep CLI output formatting in command or render modules, not in persistence helpers. Use `src/render.rs` for shared quoting, changed flag text, multiline blocks, near-match errors, and text diffs. Use focused command-family modules such as `src/commands/context.rs` for command-local snapshots and formatting. Commands with text and JSON output should construct one typed report before selecting a renderer, as `src/commands/prime.rs` does.
- Classify every `Commands` variant in the exhaustive `CliDispatch::from` match in `src/lib.rs`. Standalone commands own specialized setup, database commands use the shared SQLite path, and TUI commands use the TUI path.
- Keep TUI database access in `src/tui/store/`; keep `src/tui/ui/` rendering-only. Keep tabular task-list view modeling and hit testing in `src/tui/ui/task_list/`. Keep column grouping and navigation in `src/tui/columns.rs`, and use one shared layout from `src/tui/ui/columns.rs` for column rendering and hit testing.
- Keep release discovery, cache policy, install ownership, archive verification, and executable replacement in `src/update/`. Keep CLI update presentation in `src/commands/self_update.rs` and TUI update flow state, cancellation, and overlay coordination in `src/tui/app_update.rs`. Never replace package-manager-owned executables or request elevated privileges.
- Treat TUI column names as presentation. Column configuration must partition the six fixed semantic statuses exactly once so tasks remain visible without changing CLI, queue, readiness, dependency, or sync semantics.
- Derive TUI task list filters, query mode, and render mode from `TaskViewState`; do not keep parallel project, status, view, or queue-sort state.
- Keep live search preview state transitions and worker ownership in `SearchController` in `src/tui/app_search.rs`; keep search overlay coordination in `App` helpers, search read-model behavior in `crates/aven-core/src/query/`, and overlay rendering in `src/tui/ui/overlays/search.rs`.
- Keep natural-add mode, configuration, worker handles, pending and ready states, cancellation, and polling transitions in `IntakeController` in `src/tui/app_intake.rs`; keep authoring overlay effects in `src/tui/app_authoring.rs` and process construction in `src/tui/natural_add_runtime.rs`.
- Treat project selection in the TUI as scope. Project scope must not be modeled as a filter modifier or view.
- TUI overlays carry `OverlayRoute`; behavior resolves through `OverlayRoute::descriptor` and never depends on title text. Titles are render-only chrome.
- TUI shortcuts use intent prefixes in the command catalog. Navigation and scope use `g`, named views use `v`, composable filters use `f`, ordering uses `o`, editable task fields use `e`, other selected-task actions use `t`, project administration uses `p`, label administration uses `L`, conflicts use `c`, and config uses `C`.
- Overlay dialogs should use shared helpers in `src/tui/ui/dialog.rs` for title edges, frame clearing, background, border, and footer hint styling.
- Overlay behavior tests live in the overlay module they exercise under `src/tui/overlay/`; the facade in `src/tui/overlay.rs` stays focused on module wiring and exports.
- Record a TUI undo entry for completed TUI mutations unless the action is undo itself; pending TUI undo entries are valid only within the current `TuiStore` lifecycle and are cleared on store startup.
- Do not log auth tokens, raw sync payloads, task descriptions, note bodies, user-authored labels or project names, or secret config values.
- Keep epic membership represented by `tasks.is_epic` and `task_epic_links`. Dependency read models represent ordering only. JSON task surfaces include `is_epic`, `epic_parent`, and `epic_children` so agents can keep membership separate from blockers.
- Keep sync protocol changes aligned across `crates/aven-core/src/sync/wire.rs`, `crates/aven-core/src/sync/persistence.rs`, `crates/aven-core/src/sync/apply/`, `src/sync/server.rs`, `src/sync/client.rs`, and `src/daemon.rs`. Core validation and persistence must match the application transport loops.
- Keep bounded sync limits explicit: `MAX_PUSH_BATCH` bounds client push pages, `MAX_PULL_BATCH` bounds server pull pages, and `DAEMON_SYNC_PAGE_BUDGET` bounds daemon work per wake.
- Keep cursor semantics based on `server_seq`. Pull pages are ordered by increasing `server_seq`; response cursors equal the last returned `server_seq` or the request cursor for an empty page; local `sync_cursor` advances only after a validated page applies successfully.
- Keep daemon sync privacy-safe and budget-aware. Daemon logs and stdout include counts, cursor, completion, and page count without user content, and incomplete rounds schedule prompt follow-up sync work.
- Route successful local mutation wake attempts through `daemon::wake_if_enabled`, which owns the sync-enabled condition, wake-address resolution, and wake logging.

## Change routing

| Change | Start here | Also check | Tests |
| --- | --- | --- | --- |
| Add or change a CLI command | `src/cli.rs`, `src/lib.rs`, the relevant `src/commands/` family module | `src/commands.rs` facade exports, `src/operations/` for writes, `src/input.rs` for text input, `src/render.rs` for shared output helpers, `src/task_render.rs` for task output | focused `tests/cli_*.rs` |
| Change task detail or context output | `crates/aven-core/src/query/details.rs`, `src/commands/context.rs`, `src/task_render.rs` | `crates/aven-core/src/refs.rs`, `crates/aven-core/src/query/dependencies.rs`, `src/commands.rs` exports | focused context and show CLI tests, query detail tests, or `cargo check` |
| Add a task scalar field | core migration, `crates/aven-core/src/types.rs`, `crates/aven-core/src/task_fields.rs`, `crates/aven-core/src/mutation.rs` | `crates/aven-core/src/operations/tasks.rs`, `crates/aven-core/src/sync/apply/task.rs`, `crates/aven-core/src/sync/apply/conflict.rs`, `crates/aven-core/src/sync/wire.rs`, core queries, CLI and TUI renderers | sync, conflict, CLI, and TUI tests |
| Add task dependency relations | `crates/aven-core/src/operations/dependencies.rs`, `crates/aven-core/src/query/dependencies.rs` | `src/commands.rs`, `src/task_render.rs`, `crates/aven-core/src/sync/apply/dependency.rs`, `src/sync/server.rs` | `tests/cli_dependencies.rs`, `tests/cli_sync.rs` |
| Add or change epic membership | `crates/aven-core/src/operations/epics.rs`, `crates/aven-core/src/task_enrichment.rs`, `crates/aven-core/src/query/tasks.rs` | `src/commands.rs`, `src/tui/store/epics.rs`, `src/tui/ui/task_list/`, `crates/aven-core/src/sync/apply/epic.rs`, `src/skill.md` | `tests/cli_epics.rs`, TUI store tests, query tests, sync tests |
| Change task list, filters, sorting, search read model, or refs | `crates/aven-core/src/query/`, `crates/aven-core/src/query.rs`, `crates/aven-core/src/refs.rs`, `crates/aven-core/src/queue.rs` | CLI list and search rendering, `src/tui/store/types.rs`, `src/tui/store/view.rs`, indexes | query unit tests, `tests/tui_query.rs`, `tests/sqlite_read_path_indexes.rs`, focused CLI tests |
| Change TUI task-list rendering or hit testing | `src/tui/ui/task_list.rs`, `src/tui/ui/task_list/view_model.rs`, `src/tui/ui/task_list/hit_test.rs` | `src/tui/store/view.rs`, `src/tui/store/types.rs`, task display helpers, mouse event dispatch | `src/tui/ui/task_list.rs` module tests, `src/tui/app_tests.rs`, focused TUI tests |
| Change configurable TUI columns | `src/config.rs`, `src/tui/columns.rs`, `src/tui/ui/columns.rs` | `src/tui/store/types.rs`, `src/tui/app_navigation.rs`, view commands, sidebar, header, mouse dispatch | config tests, `src/tui/columns.rs` tests, column UI tests, focused app tests |
| Add or change the TUI recent actions view | `crates/aven-core/src/query/recent_actions.rs`, `src/tui/ui/recent_actions.rs` | `crates/aven-core/src/change_log.rs` operation names, `src/tui/store.rs`, `src/tui/store/sidebar.rs`, view commands in `src/tui/event/catalog.rs` | `cargo check`, focused TUI store or app tests |
| Change TUI search flow | `src/tui/app_search.rs` | `src/tui/app_dispatch.rs`, `src/tui/overlay/`, `src/tui/ui/overlays/search.rs`, `crates/aven-core/src/query/` search helpers | `src/tui/app_tests.rs`, focused search and query tests |
| Add or change the TUI add-task composer | `src/tui/overlay/state.rs`, `src/tui/overlay/handlers.rs`, `src/tui/ui/overlays/add_task.rs` | `src/tui/app_authoring.rs`, `src/tui/app_dispatch.rs`, picker and label child controls, `src/tui/natural_add_runtime.rs` | `cargo test add_task` |
| Add or change a TUI action | `src/tui/event/catalog.rs`, `src/tui/app_dispatch.rs`, focused `src/tui/app_*.rs` modules | flow helpers, overlays, store module, undo, `src/tui/natural_add_runtime.rs` for natural-add worker setup | `src/tui/app_tests.rs`, `src/tui/store/tests.rs`, overlay module tests |
| Change CLI or TUI update behavior | `src/update/`, `src/commands/self_update.rs`, `src/tui/app_update.rs` | `src/lib.rs`, `src/tui/app_lifecycle.rs`, `src/tui/overlay/`, `src/tui/ui/overlays/update.rs`, `src/tui/ui/header.rs`, release artifact names in `.github/workflows/release.yml` | updater unit tests, CLI surface checks, focused app, overlay, and header tests |
| Add or change TUI overlay behavior | `src/tui/overlay.rs`, `src/tui/overlay/`, `src/tui/app_overlay_submit.rs` | `OverlayRoute::descriptor`, input helpers, state builders, view projection, submit dispatch, module-local tests | `cargo test tui::overlay` |
| Add or change TUI overlay rendering | `src/tui/ui/overlays.rs`, `src/tui/ui/overlays/` | overlay view models, shared dialog helpers, input helpers, theme | overlay rendering tests in `src/tui/ui/overlays/tests.rs` |
| Change sync protocol or conflict handling | `crates/aven-core/src/sync/wire.rs`, `crates/aven-core/src/sync/persistence.rs`, `crates/aven-core/src/sync/apply/` | `src/sync/server.rs`, `src/sync/client.rs`, `src/daemon.rs`, core mutation and field helpers, migrations if persisted | `tests/cli_sync*.rs`, `tests/cli_conflicts.rs`; focused bounded-sync checks include `cargo test --test cli_sync sync_server_returns_bounded_pull_pages`, `cargo test --test cli_sync sync_client_drains_paged_remote_changes`, `cargo test --test cli_sync sync_client_drains_paged_local_changes`, `cargo test --test cli_sync current_protocol_version_sync_succeeds`, and `cargo test --test cli_sync wrong_response_protocol_version_is_rejected` |
| Add or change backup, export, or import commands | `src/cli.rs`, `src/lib.rs`, `src/commands/data_safety/mod.rs`, `crates/aven-core/src/data_safety.rs` | core database open and migration behavior in `crates/aven-core/src/db.rs`, doctor presentation in `src/commands/doctor.rs` | `tests/cli_data_safety.rs`, `tests/cli_doctor.rs` |
| Change config, workspace, or project path routing | `src/config.rs`, `src/config_edit.rs`, `src/workspaces.rs`, `src/projects.rs` | config text writes, managed-entry edits, doctor, project commands, TUI workspace and project pickers | `tests/cli_config_daemon.rs`, `tests/cli_workspaces.rs`, `tests/cli_doctor.rs` |
| Change natural-language task intake or agent primer | `src/task_intake.rs`, `src/skill.md`, `src/commands/skill.rs` | config schema, `aven prime`, add-task flows, `src/tui/app_intake.rs` for TUI intake state and lifecycle, `src/tui/natural_add_runtime.rs` for TUI background worker setup | `tests/cli_task_intake.rs`, `tests/cli_skill.rs`, focused add-task tests |
| Change logging | `src/logging.rs` and call sites | safe field policy in guardrails | `tests/cli_logging.rs` |

## Common feature checklists

### Add a CLI command

1. Add args and a `Commands` variant in `src/cli.rs`.
2. Classify the variant in the exhaustive `CliDispatch::from` match in `src/lib.rs` and add its command metadata in `src/command_metadata.rs`.
3. Add handling to the corresponding standalone, database, or TUI dispatch match in `src/lib.rs`.
4. Add command handling and output formatting to the focused command-family module under `src/commands/`, then export the entry point from `src/commands.rs`.
5. Put transactional business logic in `crates/aven-core/src/operations/` and expose it through `aven_core::db::Database`.
6. Add integration tests in `tests/`.

### Add a task scalar field

1. Create a migration under `crates/aven-core/migrations/` with `just migration-new <lower_snake_name>`.
2. Update `Task` in `crates/aven-core/src/types.rs` and row mapping in core refs or query code.
3. Update create payloads, update DTOs, core operations, and mutation validation.
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

1. Update the wire contract and shared constants in `crates/aven-core/src/sync/wire.rs`.
2. Keep request preparation, response validation, acknowledgements, and cursor advancement aligned in `crates/aven-core/src/sync/persistence.rs`.
3. Keep the HTTP client loop in `src/sync/client.rs` aligned with core page preparation and application.
4. Keep the Axum handler in `src/sync/server.rs` aligned with core sequence allocation, duplicate push acknowledgements, and bounded pull pages.
5. Keep remote apply in `crates/aven-core/src/sync/apply/` transaction-safe and cursor-safe.
6. Keep daemon work in `src/daemon.rs` bounded by `DAEMON_SYNC_PAGE_BUDGET`, with incomplete rounds scheduling follow-up sync.
7. Use focused validation commands: `cargo test --test cli_sync sync_server_returns_bounded_pull_pages`, `cargo test --test cli_sync sync_client_drains_paged_remote_changes`, `cargo test --test cli_sync sync_client_drains_paged_local_changes`, `cargo test --test cli_sync current_protocol_version_sync_succeeds`, `cargo test --test cli_sync wrong_response_protocol_version_is_rejected`, and `cargo test --test cli_daemon_sync daemon_syncs_large_backlog_across_budgeted_rounds`.

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
