# Architecture

`atm` is a local-first task manager implemented as a single Rust crate and binary. It provides a CLI, TUI, SQLite persistence, an HTTP sync server, and a local daemon wake path.

## Crate layout

- `src/main.rs`: Tokio entrypoint that calls `atm::run_cli()`.
- `src/lib.rs`: top-level module wiring, command dispatch, database opening, TUI launch, daemon wake after successful mutations.
- `src/cli.rs`: Clap argument and subcommand definitions.
- `src/commands.rs`: user-facing CLI command handlers.
- `src/operations.rs`: transactional business operations used by CLI and TUI.
- `src/mutation.rs`: field-level task mutations, validation, change recording, and field version updates.
- `src/db.rs`: SQLite connection setup, migrations, metadata, sync helpers, and conflict helpers.
- `src/query.rs`: read models for task lists, project lists, sidebar counts, filters, sorting, and conflicts.
- `src/sync.rs`: HTTP sync client, Axum sync server, remote change application, and conflict creation.
- `src/daemon.rs`: periodic sync loop and local wake listener.
- `src/config.rs`: config file loading, default paths, and environment or CLI override resolution.
- `src/projects.rs`, `src/labels.rs`, `src/refs.rs`: project, label, and task reference helpers.
- `src/render.rs`, `src/task_render.rs`: text output formatting.
- `src/tui/`: Ratatui application, input handling, store, rendering, overlays, theme, and widgets.
- `migrations/`: SQLite schema migrations.
- `tests/`: integration-heavy CLI, sync, daemon, conflict, schema, and TUI smoke coverage.

## Command flow

1. `main` starts the Tokio runtime and calls `run_cli`.
2. Clap parses `Cli` and `Commands` in `src/cli.rs`.
3. `src/lib.rs` handles special commands first:
   - `server` starts the Axum sync server.
   - `config` runs without opening the task database.
   - `daemon run` resolves config and starts the daemon.
4. Other commands load config, resolve the database path, open SQLite, and dispatch to command handlers.
5. `tui` hands the open pool to `tui::run`.
6. Successful mutating commands wake the daemon when sync is enabled and a loopback wake address is configured.

CLI commands cover task add, show, list, update, note, delete, restore, projects, labels, project paths, conflict list or show or resolve, config, daemon, server, sync, and TUI.

## Persistence model

SQLite is the only persistence layer. `open_db` enables WAL, foreign keys, a single connection, and automatic migrations. The initial migration defines materialized domain tables plus sync bookkeeping:

- Domain tables: `tasks`, `projects`, `labels`, `project_paths`, `task_labels`, `notes`.
- Sync tables: `changes`, `field_versions`, `conflicts`.
- Metadata table: `meta` stores `client_id`, `sync_cursor`, `local_seq`, and sync server URL.

`Task` and `Project` in `src/types.rs` are the core records. Task state uses string fields for `status` and `priority` plus a `deleted` boolean. Read paths wrap records into list and sidebar DTOs in `src/query.rs`.

## Mutation and invariants

Mutations flow through `src/mutation.rs` or higher-level operations in `src/operations.rs`:

1. Validate fixed-choice fields such as status and priority.
2. Reject writes to fields with unresolved conflicts.
3. Read the current field version.
4. Apply the field update to `tasks`.
5. Append a `changes` row.
6. Update `field_versions`.

Task creation writes the task, labels, a `create_task` change, and initial field versions. Updates are field-by-field. Delete is a soft-delete by setting `deleted`; restore sets it back.

Important invariants:

- Projects have unique keys and prefixes.
- `tasks.project_key` points at a valid project.
- Labels must exist before assignment.
- Task refs are abbreviated IDs with optional project-prefix hints, and ambiguous refs are rejected.
- Sync server URL is pinned per database.
- Local changes have `changes.server_seq IS NULL` until accepted by a server.
- `sync_cursor` advances only after remote changes are applied.

## Sync and integration

The external integration boundary is HTTP sync plus local UDP wake signaling. No GitHub, Taskwarrior, or generic import/export integration exists in this codebase.

Local writes append to `changes`. Manual sync performs this sequence:

1. Resolve the server URL from CLI, environment, or config.
2. Load unsynced local changes where `server_seq IS NULL`.
3. POST `/sync` with `client_id`, `after`, and pending changes.
4. Apply returned remote changes transactionally.
5. Update `sync_cursor`.

The server is a minimal Axum `POST /sync` endpoint. Public bind addresses are rejected unless `--unsafe-public-bind` is set. Daemon wake addresses must be loopback.

Conflicts are detected per task field. If a remote change base version does not match the current field version, sync records a `conflicts` row instead of overwriting. Local writes to conflicted fields are blocked until resolution.

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

The TUI store calls the same operations and mutation helpers as the CLI, so TUI edits preserve change log, field version, conflict, and validation behavior.

## Testing and development

The repository uses `just` as the main development entrypoint:

- `just check`
- `just pre-commit`
- `just test`
- `just sqlx-prepare`
- `just sqlx-check`
- `just run -- ...`

The pre-commit path runs formatting checks, clippy, tests, and SQLx offline query validation. Local project instructions say cargo format and tests run automatically on commit.

Tests live mostly in `tests/` and use `tests/common/mod.rs` for temp directories, config files, databases, spawned daemons or servers, and stdout or stderr assertions. There is no dedicated fixtures directory; tests usually create data programmatically or through temp files.

When adding features, follow the existing layering:

1. Define CLI input in `src/cli.rs`.
2. Wire dispatch in `src/lib.rs`.
3. Put user-facing command output in `src/commands.rs`.
4. Put transactional business logic in `src/operations.rs`.
5. Put low-level persistence helpers in `src/db.rs` or focused modules.
6. For mutations, record changes and keep `field_versions` aligned.
7. For schema or SQLx query changes, add migrations and update `.sqlx` metadata.
