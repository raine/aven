# Architecture

`atm` is a local-first task manager implemented as a single Rust crate and binary. It provides a CLI, TUI, SQLite persistence, an HTTP sync server, and a local daemon wake path. This document is a roadmap for coding agents. For operator-facing usage rules, also read `docs/agent-usage.txt`.

## Crate layout

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | Tokio entrypoint that calls `atm::run_cli()`. |
| `src/lib.rs` | Module wiring, command dispatch, database opening, TUI launch, daemon wake after successful CLI mutations. |
| `src/logging.rs` | Tracing subscriber initialization from `ATM_LOG` and `ATM_LOG_FILE`. |
| `src/cli.rs` | Clap argument and subcommand definitions. |
| `src/commands.rs` | User-facing CLI command handlers and output formatting calls. |
| `src/operations.rs` | Transactional business operations used by CLI and TUI. |
| `src/mutation.rs` | Field-level task mutations, scalar conflict checks, change recording, and field version updates. |
| `src/db.rs` | SQLite connection setup, migrations, metadata, sync helpers, and conflict helpers. |
| `src/query.rs` | Read models for task lists, project lists, sidebar counts, filters, sorting, and conflicts. |
| `src/sync.rs` | HTTP sync client, Axum sync server, wire types, remote change application, and conflict creation. |
| `src/daemon.rs` | Periodic sync loop and local wake listener. |
| `src/config.rs` | Config file loading, default paths, and environment or CLI override resolution. |
| `src/workspaces.rs` | Workspace records, active workspace resolution, routing, and management helpers. |
| `src/choices.rs` | Canonical task statuses, priorities, and choice validation. |
| `src/ids.rs` | UTC timestamp helper and 80-bit Crockford Base32 ID generation. |
| `src/input.rs` | Inline, file, or stdin text input handling for descriptions and notes. |
| `src/projects.rs` | Project key normalization, prefix generation, lookup, creation, path inference, and path mappings. |
| `src/labels.rs` | Label normalization, lookup, creation, and near-match validation. |
| `src/refs.rs` | Task ref parsing, ref resolution, and display ref generation. |
| `src/render.rs`, `src/task_render.rs` | Generic text helpers and task-specific CLI rendering. |
| `src/signals.rs` | Shutdown signal helper for long-running processes. |
| `src/tui/` | Ratatui application, input handling, store, rendering, overlays, theme, and widgets. |
| `src/undo.rs` | Persistent TUI undo journal, guarded inverse payloads, and apply helpers. |
| `migrations/` | SQLite schema migrations. |
| `tests/` | Integration-heavy CLI, sync, daemon, conflict, schema, and TUI smoke coverage. |

## Command flow

1. `main` starts the Tokio runtime and calls `run_cli`.
2. Clap parses `Cli` and `Commands` in `src/cli.rs`.
3. Tracing initializes after CLI parsing and writes to `ATM_LOG_FILE` when set, otherwise `$XDG_STATE_HOME/atm/atm.log` or `~/.local/state/atm/atm.log`. `ATM_LOG` controls the filter and defaults to `atm=info`. Log fields use IDs, counts, operation names, and safe paths, and must not include auth tokens, raw sync payloads, task descriptions, note bodies, user-authored labels or project names, or secret config values.
4. `src/lib.rs` handles special commands first:
   - `server` starts the Axum sync server.
   - `config` runs without opening the task database.
   - `daemon run` resolves config and starts the daemon.
5. Other commands resolve configuration, open SQLite, and dispatch to handlers.
6. Workspace-scoped commands resolve an active workspace before dispatch.
7. `tui` hands the open pool to `tui::run`.
8. Successful mutating CLI commands wake the daemon when sync is enabled and a loopback wake address is configured.

`--db` selects the database path but commands still load config so workspace routes, workspace defaults, sync settings, and daemon settings remain available. Active workspace resolution uses `--workspace`, then the longest matching config route, then `workspace.default`, then the built-in default workspace, then the only workspace in the database. Commands fail with `workspace-required` only when the default workspace is unavailable and no active workspace can be inferred.

CLI commands cover task add, show, list, update, note, delete, restore, projects, labels, project paths, workspace management, conflict list or show or resolve, config, doctor, daemon, server, sync, and TUI.

## Persistence model

SQLite is the only persistence layer. `open_db` enables WAL, foreign keys, a single connection, and automatic migrations. The initial migration defines materialized domain tables plus sync bookkeeping:

- Domain tables: `workspaces`, `tasks`, `projects`, `labels`, `project_paths`, `task_labels`, `notes`.
- Sync tables: `changes`, `field_versions`, `conflicts`.
- Metadata table: `meta` stores `client_id`, `sync_cursor`, `local_seq`, and sync server URL.
- Local TUI table: `tui_undo_entries` stores inverse operations for TUI mutations.

`Task` and `Project` in `src/types.rs` are the core records. They carry `workspace_id`, and workspace-scoped tables include `workspace_id` in uniqueness and lookup paths. Task state uses string fields for `status` and `priority` plus a `deleted` boolean. Read paths wrap records into list and sidebar DTOs in `src/query.rs`.

Many invariants are application-enforced rather than database-enforced. Do not write domain tables directly unless the operation intentionally bypasses sync and validation. Prefer `operations.rs`, `mutation.rs`, project helpers, label helpers, and ref helpers.

## Domain rules

- Status values are `inbox`, `backlog`, `todo`, `active`, `done`, and `canceled`.
- Priority values are `none`, `low`, `medium`, `high`, and `urgent`.
- New entity IDs come from `crate::ids::new_id()`, which returns 16 Crockford Base32 characters from 80 random bits.
- Timestamps come from `crate::ids::now()` and are UTC strings.
- Projects normalize names into keys with lowercase words joined by `-`.
- Project prefixes are generated to be unique and are display context, not task identity.
- Labels normalize before storage and must exist before assignment.
- Task refs resolve by ID suffix, optionally qualified as `PREFIX-SUFFIX`.
- Typed task ref suffixes must be at least 3 characters. Display refs use at least 4 suffix characters and lengthen to disambiguate current tasks.
- `O` normalizes to `0`, and `I` or `L` normalize to `1` when resolving refs.

## Mutation and invariants

Scalar task field mutations flow through `src/mutation.rs` or higher-level operations in `src/operations.rs`:

1. Validate fixed-choice fields such as status and priority.
2. Reject writes to scalar fields with unresolved conflicts.
3. Read the current field version.
4. Apply the field update to `tasks`.
5. Append a `changes` row with the previous field version as `base_version`.
6. Update `field_versions`.

Task creation writes the task, labels, a `create_task` change, and initial field versions for scalar fields. Delete is a soft-delete by setting `deleted`; restore sets it back.

Important invariants:

- Workspace keys are unique across the database.
- Projects have unique keys and prefixes within a workspace.
- `tasks.project_key` should point at a valid project in the same workspace.
- Task refs must reject ambiguous suffixes within the active workspace.
- Sync server URL is pinned per database.
- Local changes have `changes.server_seq IS NULL` until accepted by a server.
- `sync_cursor` advances only after remote changes are applied.

## Sync semantics

The external integration boundary is HTTP sync plus local UDP wake signaling. No GitHub, Taskwarrior, or generic import/export integration exists in this codebase.

Synced operation-log entities:

- Workspaces: `create_workspace` and workspace scalar field changes.
- Projects: `create_project`.
- Labels: `create_label`.
- Tasks: `create_task`, scalar `set_field`, and `resolve_field`.
- Task labels: `label_add` and `label_remove`, merged without field-version conflicts.
- Notes: `note_add`, append-only.

Workspace-scoped sync payloads include `workspace_id` and `workspace_key`. The remote apply path accepts older default-workspace payloads for compatibility and applies scoped records into their owning workspace.

Local-only data:

- Config files and environment overrides.
- Project path mappings in `project_paths`.
- TUI view, filter, selection, overlay, and sort state.
- TUI undo entries in `tui_undo_entries`.

Conflict-protected scalar task fields are `title`, `description`, `project`, `status`, `priority`, and `deleted`. Labels and notes sync through operation records but do not use scalar field conflict protection.

Manual sync performs this sequence:

1. Resolve the server URL from CLI, environment, or config.
2. Load unsynced local changes where `server_seq IS NULL`.
3. POST `/sync` with `protocol_version`, `client_id`, `after`, and pending changes.
4. Apply returned remote changes transactionally.
5. Update `sync_cursor`.

The server is an Axum `POST /sync` endpoint using the `SyncRequest`,
`SyncResponse`, and `ChangeWire` JSON shapes in `src/sync.rs`. Requests and
responses include `protocol_version`, and both peers require an exact match with
`SYNC_PROTOCOL_VERSION` before applying changes. It assigns server sequence
numbers and persists changes.

Startup classifies the bind address as loopback, private, or public and prints
`scope=<scope>` on the listening line. Loopback binds can run without a token
for local testing. Private binds require a configured `sync.auth_token`. Public
binds require `--unsafe-public-bind`, a configured token, and print a warning
that TLS or a reverse proxy is needed. When a token is configured, clients send
`Authorization: Bearer <token>` and the server rejects unauthorized `/sync`
requests before applying changes.

The server validates incoming operation names, entity types, payload shapes,
fixed-choice values, sync ID formats, and server-owned fields before appending
changes to its log. It does not require referenced entities to exist on the
server because offline batches can contain related operations that arrive
together. Daemon wake addresses must be loopback.

If a remote scalar change base version does not match the current field version, sync records a `conflicts` row instead of overwriting. If an unresolved conflict already exists for that task field, another remote change for the field is also rejected into conflict handling.

## TUI architecture

`src/tui/mod.rs` constructs `App`, initializes Ratatui, runs the app loop, and restores the terminal on exit.

The TUI is split into these layers:

- `app.rs`: application state, event loop, focus, selection, overlays, authoring flows, conflict flows, action execution, and refresh cadence.
- `event.rs`: shared command catalog, key sequences, command lookup, shortcut resolution, action lifecycle, and help metadata.
- `store.rs`: database-backed TUI state and operations. It owns task lists, projects, labels, sidebar counts, filters, sorting, active view, and refresh time.
- `overlay.rs`: reusable text input, multiline input, picker, confirm, search, command, detail, help, and text panel state machines.
- `ui.rs`: pure Ratatui rendering for header, sidebar, task list, preview, footer, overlays, command palette, help, and prefix hints.
- `widgets.rs`: small cell helpers such as priority icons and title conflict markers.
- `theme.rs`: colors and style helpers.

The app loop draws the current view, polls keyboard input every 250 ms, dispatches keys, refreshes store data every 5 seconds, and clears expired messages. Normal keys resolve through the command catalog. Capturing overlays handle their own input before normal shortcuts. Multi-key prefixes are stored in `pending_shortcut` and rendered as hints.

The TUI store calls the same operations and mutation helpers as the CLI for mutations, so TUI edits preserve change log, field version, conflict, and validation behavior. TUI query and sort state is separate from CLI list defaults.

TUI undo records one inverse operation per completed TUI mutation in `tui_undo_entries`. Entries are workspace-scoped, persist across TUI restarts, and apply through the same mutation helpers so undo effects follow normal sync semantics. Scalar field and label undos guard against stale state before applying. `:undo` and `z` dispatch to the same undo action.

To add a TUI action:

1. Add an `Action` variant in `src/tui/event.rs`.
2. Register it in the `COMMANDS` catalog with key sequences, section, lifecycle, and description.
3. Handle the action in `App::execute` in `src/tui/app.rs`.
4. Add or reuse overlay state if the action needs user input.
5. Add `TuiStore` methods for database reads or mutations.
6. Record a TUI undo entry for mutating store methods unless the action is undo itself.
7. Add tests for shortcut resolution, action dispatch, overlay routing, and store behavior.

Overlay submits are routed by overlay title strings in `App::handle_overlay_submit`. Keep title constants unique, and update tests when adding overlay flows.

## Feature checklists

### Add a CLI command

1. Add args and a `Commands` variant in `src/cli.rs`.
2. Add dispatch in `src/lib.rs`.
3. Add output and input handling in `src/commands.rs`.
4. Put transactional business logic in `src/operations.rs`.
5. Put low-level persistence helpers in `src/db.rs` or focused modules.
6. For mutating commands, record changes and keep `field_versions` aligned when scalar fields change.
7. For mutating CLI commands that should trigger prompt sync, update `command_should_wake` in `src/lib.rs`.
8. Add integration tests in `tests/`.

### Add a task scalar field

1. Add a migration for the `tasks` column and any indexes.
2. Update `Task` in `src/types.rs` and task row mapping in `src/refs.rs` or query code.
3. Update create payloads, update DTOs, and operation logic in `src/operations.rs`.
4. Update `apply_field_value` and mutation validation in `src/mutation.rs`.
5. Seed field versions during task creation if the field needs conflict protection.
6. Update sync remote apply and conflict resolution behavior in `src/sync.rs`.
7. Update list filters, sort, or display models in `src/query.rs` if needed.
8. Update CLI rendering and TUI rendering or overlays.
9. Add tests for local mutation, sync, conflict behavior, and TUI behavior if exposed there.
10. Run `just sqlx-prepare` after changing migrations or `sqlx::query!` shapes.

### Add text input to a command

Use `src/input.rs` so inline, file, and stdin sources remain mutually exclusive and error messages stay consistent.

## Testing and development

The repository uses `just` as the main development entrypoint:

- `just pre-commit`: read-only validation gate for formatting, static analysis, clippy, and tests.
- `just check`: local read-only validation gate, equivalent to `just pre-commit`.
- `just pre-merge`: deferred validation gate for SQLx metadata and build output.
- `just check-full`: local read-only gate plus deferred merge checks.
- `just clippy-fix`: explicit opt-in command for machine-applicable clippy fixes.
- `just test`: Rust test suite through `cargo nextest`, plus Rust doctests.
- `just sqlx-prepare`: regenerate SQLx offline query metadata after migrations or query shape changes.
- `just sqlx-check`: verify SQLx offline query metadata.
- `just run -- ...`: run the application.

The pre-commit hook runs `git-format-staged`, hides unstaged changes while validation runs, and suggests `just clippy-fix` if clippy reports fixable lints. Workmux runs `just pre-merge` before merging. Local project instructions say cargo format and tests run automatically on commit.

Tests live mostly in `tests/` and use `tests/common/mod.rs` for temp directories, config files, databases, spawned daemons or servers, and stdout or stderr assertions. There is no dedicated fixtures directory; tests usually create data programmatically or through temp files.

The project uses `sqlx::query!` compile-time checked queries with `.sqlx/` metadata. Never commit schema or query macro changes without regenerating and checking SQLx metadata.
