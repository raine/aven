# Architecture

`aven` is a local-first task manager implemented as one Rust crate and binary. It provides a CLI, Ratatui TUI, SQLite persistence, HTTP sync server, and local daemon wake path. This file is a sitemap for coding agents. For operator-facing command usage, also read `docs/agent-usage.txt` and `src/skill.md`.

## System map

| Layer | Owns | Start here | Rules |
| --- | --- | --- | --- |
| CLI entry and dispatch | argument parsing, command routing, config load, database open, daemon wake dispatch, coding-agent skill installation | `src/main.rs`, `src/lib.rs`, `src/cli.rs`, `src/commands.rs`, `src/commands/tasks.rs`, `src/commands/bulk_update.rs`, `src/commands/relations.rs`, `src/commands/doctor.rs`, `src/commands/context.rs`, `src/commands/prime.rs`, `src/commands/skill.rs` | `src/commands.rs` is the module and re-export facade. Command-family modules own command orchestration and command-local formatting. Business writes belong in operations or mutation helpers. |
| Write model | transactional task, project, label, conflict, config, and workspace changes | `src/operations/`, `src/mutation.rs`, `src/task_fields.rs` | Synced scalar task writes must update tasks, `changes`, and `field_versions` together. Attachment collection writes use operation-log entries and do not participate in scalar field versions. |
| Read model | task lists, task details, task search, project lists, sidebar counts, attachment metadata, filters, sorting, refs, and enrichment | `src/query.rs`, `src/query/`, `src/query/details.rs`, `src/task_enrichment.rs`, `src/refs.rs`, `src/queue.rs` | Use the task-detail model for enriched single-task reads. Keep list and search reads batch-oriented, and avoid per-row queries on list paths. Keep retrieval-style task search separate from scoped list filters. Attachment search covers filename and alt text, not bytes, hashes, paths, or attachment IDs. Attachment IDs and metadata never resolve through description Markdown. |
| Persistence | SQLite setup, migrations, sync metadata, conflict helpers, SQLx metadata | `src/db.rs`, `migrations/`, `.sqlx/` | Create migrations with `just migration-new <lower_snake_name>`. Refresh SQLx metadata after query or schema changes. |
| Attachment storage | decoded image validation, content-addressed sidecar bytes, configured PNG optimization, lazy preview cache, secure external-viewer exports, blob inventory, lifecycle policy, leases, quotas, recoverable pruning, and attachment IDs | `src/attachments/`, `src/operations/attachments.rs`, `src/config.rs` | Attachment bytes live under the resolved blob directory at `objects/sha256/<hash>`. Validation derives format and dimensions from decoded content. `src/attachments/lifecycle.rs` centralizes liveness, protection, unique-byte accounting, reservations, grace eligibility, trash recovery, and preview eviction. `src/attachments/export.rs` creates leased, extension-bearing temporary copies for OS image viewers without exposing object paths. Device-local thumbnails live under `cache/previews/` and stay outside inventory and portability flows. The `local.image_optimization` config selects byte optimization for paste sources and file attachments. Task descriptions remain user-authored Markdown, while `task_attachments` owns attachment identity and presentation. |
| Config and routing | config files, managed config text edits, path mappings, workspace resolution, project inference | `src/config.rs`, `src/config_edit.rs`, `src/workspaces.rs`, `src/projects.rs` | `src/config.rs` owns durable config text writes. Managed entry text surgery belongs in `src/config_edit.rs`. Workspace-scoped commands must resolve an active workspace before domain lookup. |
| Sync and daemon | HTTP sync client/server, wire DTOs, remote apply, launchd service management, wake-if-enabled policy, wake loop | `src/sync.rs`, `src/sync/`, `src/sync/apply/`, `src/daemon.rs`, `src/daemon/service.rs` | Wire shapes, protocol version, server validation, and remote apply semantics must evolve together. LaunchAgent ownership and plist rendering live in the daemon service module. Attachment metadata sync uses JSON change rows. Attachment bytes transfer through authenticated blob endpoints outside `/sync` payloads. |
| TUI app | launch-intent resolution, event loop, actions, overlays, store, rendering, undo, natural add runtime, platform helpers | `src/tui/store/launch.rs`, `src/tui/` | Store modules own DB access, including launch-time project, label, and task-ref resolution. UI modules render view models. Overlay routes drive behavior, not titles. Natural add worker setup belongs in `src/tui/natural_add_runtime.rs`. |
| Update delivery | cached GitHub release discovery, semantic version comparison, install ownership classification, verified direct replacement | `src/update.rs`, `src/update/` | Background checks are fail-silent and rate-limited. CLI and TUI flows own their presentation and confirmation behavior. Package-manager installations receive manager-specific guidance rather than binary replacement. |
| Shared domain helpers | IDs, choices, labels, input loading, text rendering, CLI render output, logging, fuzzy matching | `src/ids.rs`, `src/choices.rs`, `src/labels.rs`, `src/input.rs`, `src/render.rs`, `src/task_render.rs`, `src/logging.rs`, `src/fuzzy.rs`, `src/types.rs` | Reuse canonical helpers instead of duplicating validation, display, diff, or parsing rules. Attachment section placeholders for CLI and TUI read surfaces come from `src/task_render.rs`. |
| Tests and tooling | CLI integration tests, TUI/store tests, overlay module tests, SQL index checks, just tasks | `tests/`, `src/tui/*tests.rs`, `src/tui/overlay/`, `justfile` | Add focused tests near the subsystem, run relevant focused tests during development, and rely on the pre-push hook for the full gate. |

## Runtime flows

### CLI command flow

1. `src/main.rs` starts Tokio and calls `aven::run_cli()`.
2. `src/cli.rs` parses `Cli` and `Commands`.
3. `src/lib.rs` classifies every parsed command into the explicit standalone, database, or TUI dispatch class through an exhaustive `Commands` match. Each typed dispatch path performs its required setup, then routes through the `src/commands.rs` facade to focused command-family modules under `src/commands/`. `aven update` dispatches to `src/commands/self_update.rs` without opening the task database.
4. Mutating commands call operations or mutation helpers, then dispatch through the daemon wake-if-enabled policy.

### Attachment lifecycle

1. `src/commands/attachments.rs` parses CLI attachment commands, resolves refs in the active workspace, accepts optional declared media type, and formats metadata-only text or JSON output.
2. `src/operations/attachments.rs` validates decoded content, derives canonical media type and dimensions, canonicalizes bytes for optimization-enabled sources, stores the final bytes through `src/attachments/storage.rs`, and records `attachment_add` or `attachment_delete` change rows with `field = "attachments"`. TUI paste reserves attachment identity and ordering before background preparation, while synced metadata and its SHA-256 enter SQLite only after final canonical bytes are ready. Task creation with pending attachments is sequenced by `src/operations/tasks.rs`: it prepares and durably stages every distinct object before opening one immediate transaction for task rows, inventory, attachment metadata, field versions, and change rows. Staging reports physical-object ownership so failed operations can remove only invocation-created, freshly unreferenced objects.
3. `src/config.rs` resolves `local.blob_dir`: absolute paths are used directly, relative paths are rooted beside the active SQLite database, and omitted values use `<resolved-db-path>.blobs`.
4. `src/task_enrichment.rs` and query hydration load attachment metadata with `has_blob`; show, context, JSON, search, and TUI surfaces consume that read model without reading bytes.
5. `src/sync/planner.rs` deterministically selects unique blob hashes under the shared object and byte budget. `src/sync/client.rs` uploads the selected missing local blobs before pushing the largest admissible attachment-metadata prefix, applies pulled metadata and cursor progress before local downloads, and drains interactive work through repeated bounded rounds.
6. `src/sync/server.rs` serves `/sync/blobs/missing`, `PUT /sync/blobs/<sha256>`, and `GET /sync/blobs/<sha256>` behind the same auth path as sync metadata.
7. `src/commands/data_safety/` packages the database and sidecar objects in backup archives, keeps JSON export metadata-only with `blobs_included: false`, marks imported blob inventory unavailable, and exposes attachment consistency checks for `doctor`.
8. `src/attachments/lifecycle.rs` persists final-reference transitions, classifies live and protected hashes, enforces unique-original quotas, coordinates staging, transfer, read, backup, and upload leases, and prunes through recoverable `trash/` moves with an immediate policy recheck. Daemon sync, server sync work, explicit prune, and quota admission run bounded maintenance. `aven doctor` consumes the same classification policy. Preview cache eviction has an independent byte bound.
9. `src/tui/markdown.rs` renders task descriptions as ordinary Markdown, and `src/tui/ui/detail.rs` renders enriched live attachment rows plus app-owned pending and failed attachment view state in a dedicated section. `src/tui/attachment_controller.rs` owns bounded asynchronous paste work, reserved task identity and ordering, completion states, and shutdown cleanup. TUI detail rendering reserves inline preview rows for available local image bytes when `src/tui/inline_images.rs` detects a supported terminal backend, and `src/tui/app_lifecycle.rs` emits the terminal image protocol after Ratatui draws the frame. `src/tui/app_attachments.rs` routes task-detail and add-task composer image paste into structured attachment operation paths used by the CLI.
10. `src/tui/app_dispatch.rs` routes locally available image attachments to either the terminal preview or the OS default viewer. `src/attachments/export.rs` validates current attachment state, holds a read lease while creating and launching a secure temporary copy with a media-appropriate extension, and `src/tui/platform.rs` selects the platform launcher without shell interpolation. The TUI retains a bounded set of exports so viewer processes can read them and drops the copies during eviction or shutdown.

### TUI flow

1. `src/tui/store/launch.rs` resolves CLI launch arguments into one complete `TaskViewState` and startup action before `src/tui/mod.rs` initializes Ratatui and constructs `App`. Direct task targets use workspace-scoped Search state with one task ID so deleted and otherwise hidden tasks remain visible.
2. Input resolves through the command catalog in `src/tui/event/` unless a capturing overlay handles it first.
3. `App` holds TUI flow controllers and simple UI state. Focused `src/tui/app_*.rs` modules coordinate feature flows. `src/tui/app_onboarding.rs` owns first-launch eligibility and the continuous onboarding timeline, `src/tui/app_lifecycle.rs` schedules its redraws and input skip behavior, and `src/tui/ui/splash.rs` projects its logo reveal, dimming, and afterglow state into the SVG-derived terminal raster. Durable onboarding policy lives in `src/tui/store/onboarding.rs`. `SearchController` in `src/tui/app_search.rs` owns live search preview state and worker lifecycle, `IntakeController` in `src/tui/app_intake.rs` owns natural-add mode, configuration, worker lifecycle, and pending-to-ready transitions, `AttachmentController` in `src/tui/attachment_controller.rs` owns bounded task-detail image preparation and commit work, `PreviewController` in `src/tui/preview_controller.rs` owns bounded terminal attachment preview work, caching, retries, and stale-result rejection, and `UpdateController` owns update flow resources and state. Attachment preparation and preview generation use separate two-permit blocking pools so preview work cannot hold interactive attachment capacity.
4. `TaskViewState` is the source of truth for TUI task lists. It carries scope, view, filter modifiers, flat order, and direction, then derives query filters, query mode, and render mode. `src/tui/columns.rs` derives column grouping and navigation from the configured fixed-status partition while retaining global task indexes. `src/tui/ui/task_list/` owns tabular task-list view models and hit testing, and `src/tui/ui/columns.rs` owns shared column layout, rendering, and hit testing. Spotlight search renders live overlay results from the task search read model and stores matched task IDs in the submitted search view. The recent actions view reads the synced change log through `src/query/recent_actions.rs` and renders through `src/tui/ui/recent_actions.rs` without modeling actions as task rows.
5. Store modules call the same operations, mutation, and query helpers as the CLI.
6. Natural-language add flow coordination lives in `src/tui/app_authoring.rs`; `IntakeController` in `src/tui/app_intake.rs` owns intake configuration, add-task-only mode, background state, cancellation, and polling transitions; background worker command construction, environment propagation, process setup, and log path selection live in `src/tui/natural_add_runtime.rs`. The add-task overlay state owns the visible draft, six-field focus, validation, and integrated metadata, help, and discard-confirmation child modes, so child controls preserve text cursor state.
7. `src/tui/ui.rs` and `src/tui/ui/` render state. Task detail renders live enriched attachment metadata and app-owned pending or failed rows after the user-authored Markdown description. Add-task composer image paste records structured pending attachments separately from its text fields. Inline terminal previews reserve geometry from attachment metadata, load and encode validated cached PNG thumbnails through `src/tui/preview_controller.rs`, and reject stale background results. `src/tui/app_lifecycle.rs` polls attachment completion, emits previews after frame draws, owns placement cleanup and terminal writes, and waits for in-flight attachment commits during orderly shutdown. `src/tui/app_attachments.rs` handles task-detail and add-task image paste, and rendering code does not touch the database or filesystem.
8. TUI update coordination lives in `src/tui/app_update.rs`; `src/update/` owns release discovery, cache scheduling, install classification, verification, and direct executable replacement. Update availability renders as a non-modal header badge, while explicit checks and installs use overlays.

### Sync flow

1. Local mutations append operation-log rows in `changes`.
2. Unsynced rows have `server_seq IS NULL`.
3. `src/sync/client.rs` selects the largest deterministic unsynced change prefix whose unique missing attachment blobs fit the round's remaining transfer budget, uploads those blobs under workspace quota reservations, then posts at most `MAX_PUSH_BATCH` rows with the current `sync_cursor` as `after` and `MAX_PULL_BATCH` as `pull_limit`.
4. `src/sync/wire.rs` owns the protocol version, request limits, cursor validation, response validation, and daemon sync budget constants.
5. `src/sync/server.rs` validates operation names, entity types, protocol version, batch bounds, pull limits, cursors, and payload shapes before assigning server sequence numbers.
6. The server assigns strictly increasing `server_seq` values to accepted changes, reuses existing values for duplicate pushed change IDs, returns one bounded pull page, and sets `has_more` when rows remain after the page.
7. Remote apply updates local tables transactionally, records conflicts for scalar field version mismatches, applies push acknowledgements, then advances `sync_cursor` to the response cursor.
8. `src/sync/client.rs` validates each response before applying it. Response changes must fit the requested pull limit, have strictly increasing `server_seq` values after the cursor, match the response cursor, and include one push acknowledgement for each pushed change. Pulled attachment metadata and cursor advancement commit independently of subsequent blob download success.
9. The CLI sync path drains metadata pages and attachment bytes through repeated rounds. Each round shares one 16-object, 64 MiB budget across uploads and live-attachment downloads, with a one-object progress allowance when the round has completed no transfer.
10. `src/daemon.rs` calls `run_sync_with_page_budget` with `DAEMON_SYNC_PAGE_BUDGET`; incomplete daemon rounds report counts, cursor, completion, and page count, then schedule prompt follow-up sync work.

## Data ownership

SQLite stores synced task data and local UI state. Config files store local routing and service settings.

- Synced domain tables: `workspaces`, `tasks`, `projects`, `labels`, `task_labels`, `notes`, `task_dependencies`, `task_epic_links`, `task_attachments`.
- Attachment metadata: `task_attachments` stores attachment ids, task ids, sha256, byte size, media type, filename, alt text, dimensions, create change id, and tombstone fields. Attachment metadata syncs as collection operations with `entity_type = "task"` and `field = "attachments"`.
- Local attachment inventory: `blob_inventory` records sidecar byte availability. Read models expose metadata and `has_blob`; task descriptions remain independent user-authored Markdown.
- Local attachment bytes: decoded PNG, JPEG, GIF, and WebP bytes live outside SQLite under the resolved blob directory at `objects/sha256/<hash>`. Validation detects format, derives dimensions, and fully decodes bounded frames before storage. The `local.image_optimization` values are `off`, `paste`, and `on`: `off` is the default and preserves bytes unless a CLI flag overrides it, `paste` optimizes paste sources, and `on` optimizes paste sources and file attachments. Optimization stores PNG bytes only when a smaller lossless result passes the same validation. Lazy terminal thumbnails live under the disposable `cache/previews/` tree and are excluded from inventory, sync, backup, export, and import.
- Sync and local client bookkeeping: `changes`, `field_versions`, `conflicts`, and `meta`. `meta` stores sync cursors, client identity, and durable local TUI scalars such as the onboarding version. Attachment collection operations use `changes`; scalar field conflict versions remain scoped to scalar task fields.
- Local-only config: database path, sync settings, project path mappings, directory overrides, and TUI column grouping.
- Local-only TUI state: view, filter, selection, overlay, sort state, and `tui_undo_entries`; pending undo entries are cleared when a TUI store starts.
- Backup and portability workflows: `src/commands/data_safety/` for snapshot/restore flows and `src/db.rs` for backup file naming.

`Task` and `Project` in `src/types.rs` are core records. Workspace-scoped tables include `workspace_id` in uniqueness and lookup paths. Many invariants are application-enforced rather than database-enforced, so do not write domain tables directly unless a change is intentionally bypassing sync and validation.

## Domain rules

- Status values are `inbox`, `backlog`, `todo`, `active`, `done`, and `canceled`.
- Priority values are `none`, `low`, `medium`, `high`, and `urgent`.
- New entity IDs come from `crate::ids::new_id()`, which returns 16 Crockford Base32 characters from 80 random bits.
- Timestamps come from `crate::ids::now()` and are UTC strings.
- Workspace identity uses `WorkspaceId` from `src/ids.rs` throughout domain records, queries, and mutations. CLI, config, sync payload, and export conversions validate the 16-character Crockford Base32 representation, while SQLx binds and decodes the type as SQLite text.
- Project identity uses `ProjectId` from `src/ids.rs` throughout domain records, queries, and mutations. CLI, config, sync payload, and export conversions validate the 16-character Crockford Base32 representation, while SQLx binds and decodes the type as SQLite text.
- Task identity uses `TaskId` from `src/ids.rs` throughout domain records, queries, mutations, relationships, and undo snapshots. CLI refs, sync payloads, and exports preserve validated 16-character Crockford Base32 strings, while SQLx binds and decodes the type as SQLite text.
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

- Use `src/operations/` or `src/mutation.rs` for writes that affect synced domain data.
- Use `src/query.rs`, `src/query/`, and enrichment helpers for read models.
- Keep scalar task fields aligned across validation, task rows, `changes`, `field_versions`, sync apply, and conflict resolution.
- Keep workspace scope explicit on queries and mutations that operate on user data.
- Keep config serialization and durable text writes in `src/config.rs`; keep managed-entry text transforms in `src/config_edit.rs`.
- Keep CLI output formatting in command or render modules, not in persistence helpers. Use `src/render.rs` for shared quoting, changed flag text, multiline blocks, near-match errors, and text diffs. Use focused command-family modules such as `src/commands/context.rs` for command-local snapshots and formatting. Commands with text and JSON output should construct one typed report before selecting a renderer, as `src/commands/prime.rs` does.
- Classify every `Commands` variant in the exhaustive `CliDispatch::from` match in `src/lib.rs`. Standalone commands own specialized setup, database commands use the shared SQLite path, and TUI commands use the TUI path.
- Keep TUI database access in `src/tui/store/`; keep `src/tui/ui/` rendering-only. `src/tui/store/launch.rs` is the sole database-backed boundary from raw TUI CLI arguments to resolved launch state. Keep tabular task-list view modeling and hit testing in `src/tui/ui/task_list/`. Keep column grouping and navigation in `src/tui/columns.rs`, and use one shared layout from `src/tui/ui/columns.rs` for column rendering and hit testing.
- Keep TUI attachment sections metadata-backed for committed attachments and app-state-backed for local preparation. `src/task_enrichment.rs` loads ordered live attachment metadata, `src/tui/attachment_controller.rs` owns pending and failure state without publishing a hash, `src/tui/markdown.rs` renders the user-authored description, and `src/tui/ui/detail.rs` renders attachment rows, reserves preview geometry from validated dimensions, and applies the task detail quote rail. Attachment focus eligibility depends on live image metadata and local byte availability, independently of terminal graphics support. `src/tui/preview_controller.rs` owns bounded thumbnail work, payload caching and leases, request deduplication, retries, failure suppression, and stale-result rejection. `src/tui/inline_images.rs` owns terminal preview backend detection and escape construction for iTerm2 and Kitty graphics protocols. `src/tui/app_lifecycle.rs` owns terminal writes and placement cleanup. `src/attachments/export.rs` owns secure leased copies for external viewers, while `src/tui/platform.rs` owns direct platform launcher commands. Text placeholders remain visible when previews are disabled, unsupported, obscured, pending, missing local bytes, suppressed after failure, or unavailable through tmux auto mode.
- Keep release discovery, cache policy, install ownership, archive verification, and executable replacement in `src/update/`. Keep CLI update presentation in `src/commands/self_update.rs` and TUI update flow state, cancellation, and overlay coordination in `src/tui/app_update.rs`. Never replace package-manager-owned executables or request elevated privileges.
- Treat TUI column names as presentation. Column configuration must partition the six fixed semantic statuses exactly once so tasks remain visible without changing CLI, queue, readiness, dependency, or sync semantics.
- Keep attachment bytes outside SQLite task descriptions, JSON sync change payloads, JSON export files, and default command output. Only `attachment get --output <path>` writes bytes to a caller-chosen file.
- Keep attachment metadata and byte availability separate. `task_attachments` is synced domain metadata; `blob_inventory` and sidecar files describe local byte availability.
- Keep attachment validation centralized in `src/attachments/validation.rs` and byte persistence centralized in `src/attachments/storage.rs`.
- Keep sync attachment privacy-safe. Logs and stdout include change counts, unique blob counts, byte totals, remaining counts and bytes, cursor, and completion state, not filenames, alt text, hashes, sidecar paths, raw payloads, or bytes.
- Keep backup archives as the data-safety path for local bytes. JSON export/import remains metadata-only, with imported blob inventory unavailable until bytes arrive through backup restore or sync.
- Derive TUI task list filters, query mode, and render mode from `TaskViewState`; do not keep parallel project, status, view, or queue-sort state.
- Keep live search preview state transitions and worker ownership in `SearchController` in `src/tui/app_search.rs`; keep search overlay coordination in `App` helpers, search read-model behavior in `src/query/`, and overlay rendering in `src/tui/ui/overlays/search.rs`.
- Keep natural-add mode, configuration, worker handles, pending and ready states, cancellation, and polling transitions in `IntakeController` in `src/tui/app_intake.rs`; keep authoring overlay effects in `src/tui/app_authoring.rs` and process construction in `src/tui/natural_add_runtime.rs`.
- Treat project selection in the TUI as scope. Project scope must not be modeled as a filter modifier or view.
- TUI overlays carry `OverlayRoute`; behavior resolves through `OverlayRoute::descriptor` and never depends on title text. Titles are render-only chrome.
- TUI shortcuts use intent prefixes in the command catalog. Navigation and scope use `g`, named views use `v`, composable filters use `f`, ordering uses `o`, editable task fields use `e`, other selected-task actions use `t`, project administration uses `p`, label administration uses `L`, conflicts use `c`, and config uses `C`.
- Overlay dialogs should use shared helpers in `src/tui/ui/dialog.rs` for title edges, frame clearing, background, border, and footer hint styling.
- Overlay behavior tests live in the overlay module they exercise under `src/tui/overlay/`; the facade in `src/tui/overlay.rs` stays focused on module wiring and exports.
- Record a TUI undo entry for completed TUI mutations unless the action is undo itself; pending TUI undo entries are valid only within the current `TuiStore` lifecycle and are cleared on store startup.
- Do not log auth tokens, raw sync payloads, task descriptions, note bodies, user-authored labels or project names, or secret config values.
- Keep epic membership represented by `tasks.is_epic` and `task_epic_links`. Dependency read models represent ordering only. JSON task surfaces include `is_epic`, `epic_parent`, and `epic_children` so agents can keep membership separate from blockers.
- Keep sync protocol changes aligned across `src/sync/wire.rs`, `src/sync/client.rs`, `src/sync/server.rs`, `src/sync/apply/`, and `src/daemon.rs`. Request and response validation must match the client page loop and server page construction.
- Keep bounded sync limits explicit: `MAX_PUSH_BATCH` bounds client push pages, `MAX_PULL_BATCH` bounds server pull pages, `MAX_BLOB_TRANSFER_OBJECTS` and `MAX_BLOB_TRANSFER_BYTES` bound each shared upload and download round, and `DAEMON_SYNC_PAGE_BUDGET` bounds daemon metadata work per wake.
- Keep cursor semantics based on `server_seq`. Pull pages are ordered by increasing `server_seq`; response cursors equal the last returned `server_seq` or the request cursor for an empty page; local `sync_cursor` advances only after a validated page applies successfully.
- Keep daemon sync privacy-safe and budget-aware. Daemon logs and stdout include counts, cursor, completion, and page count without user content, and incomplete rounds schedule prompt follow-up sync work.
- Route successful local mutation wake attempts through `daemon::wake_if_enabled`, which owns the sync-enabled condition, wake-address resolution, and wake logging.

## Change routing

| Change | Start here | Also check | Tests |
| --- | --- | --- | --- |
| Add or change a CLI command | `src/cli.rs`, `src/lib.rs`, the relevant `src/commands/` family module | `src/commands.rs` facade exports, `src/operations/` for writes, `src/input.rs` for text input, `src/render.rs` for shared output helpers, `src/task_render.rs` for task output | focused `tests/cli_*.rs` |
| Change task detail or context output | `src/query/details.rs`, `src/commands/context.rs`, `src/task_render.rs` | `src/refs.rs`, `src/query/dependencies.rs`, `src/commands.rs` exports | focused context and show CLI tests, query detail tests, or `cargo check` |
| Add a task scalar field | migration, `src/types.rs`, `src/task_fields.rs`, `src/mutation.rs` | `src/operations/tasks.rs`, `src/sync/apply/task.rs`, `src/sync/apply/conflict.rs`, `src/sync/wire.rs`, `src/query/`, CLI and TUI renderers | sync, conflict, CLI, and TUI tests |
| Add task dependency relations | `src/operations/dependencies.rs`, `src/query/dependencies.rs` | `src/commands.rs`, `src/task_render.rs`, `src/sync/apply/dependency.rs`, `src/sync/server.rs` | `tests/cli_dependencies.rs`, `tests/cli_sync.rs` |
| Add or change epic membership | `src/operations/epics.rs`, `src/task_enrichment.rs`, `src/query/tasks.rs` | `src/commands.rs`, `src/tui/store/epics.rs`, `src/tui/ui/task_list/`, `src/sync/apply/epic.rs`, `src/skill.md` | `tests/cli_epics.rs`, TUI store tests, query tests, sync tests |
| Change task list, filters, sorting, search read model, or refs | `src/query/`, `src/query.rs`, `src/refs.rs`, `src/queue.rs` | CLI list and search rendering, `src/tui/store/types.rs`, `src/tui/store/view.rs`, indexes | query unit tests, `tests/tui_query.rs`, `tests/sqlite_read_path_indexes.rs`, focused CLI tests |
| Add or change attachment storage, validation, or sidecar paths | `src/attachments/`, `src/config.rs`, `src/db.rs`, migrations | `src/operations/attachments.rs`, `src/commands/attachments.rs`, `src/commands/data_safety/`, `.sqlx/` | `cargo check`, focused attachment operation tests, data-safety tests |
| Add or change attachment CLI behavior | `src/cli.rs`, `src/lib.rs`, `src/commands/attachments.rs`, `src/operations/attachments.rs` | `src/attachments/`, `src/task_render.rs`, `src/skill.md`, `skills/aven/SKILL.md` | focused CLI attachment tests, `cargo check` |
| Change attachment read surfaces or search matching | `src/task_enrichment.rs`, `src/query/`, `src/task_render.rs`, `src/commands/context.rs` | `src/commands/attachments.rs`, `src/tui/markdown.rs`, `src/tui/ui/detail.rs` | focused show, context, search, and query tests |
| Change attachment data safety or doctor checks | `src/commands/data_safety/`, `src/commands/data_safety/`, `src/commands/doctor.rs` | `src/db.rs`, `src/attachments/`, backup archive helpers, export/import validation | `cargo test --test cli_data_safety`, `cargo test --test cli_doctor`, `cargo check` |
| Change attachment sync metadata or blob transfer | `src/sync/wire.rs`, `src/sync/client.rs`, `src/sync/server.rs`, `src/sync/apply/`, `src/daemon.rs` | `src/change_log.rs`, `src/attachments/`, `src/operations/attachments.rs`, data-safety blob helpers | focused `tests/cli_sync.rs`, `tests/cli_daemon_sync.rs`, `cargo check` |
| Add or change TUI launch targets | `src/cli.rs`, `src/tui/store/launch.rs`, `src/lib.rs` | `src/tui/store.rs`, `src/tui/mod.rs`, `src/tui/app.rs`, TUI guide and command reference | CLI parser tests, launch resolution tests, focused app startup tests |
| Change TUI task-list rendering or hit testing | `src/tui/ui/task_list.rs`, `src/tui/ui/task_list/view_model.rs`, `src/tui/ui/task_list/hit_test.rs` | `src/tui/store/view.rs`, `src/tui/store/types.rs`, task display helpers, mouse event dispatch | `src/tui/ui/task_list.rs` module tests, `src/tui/app_tests.rs`, focused TUI tests |
| Change configurable TUI columns | `src/config.rs`, `src/tui/columns.rs`, `src/tui/ui/columns.rs` | `src/tui/store/types.rs`, `src/tui/app_navigation.rs`, view commands, sidebar, header, mouse dispatch | config tests, `src/tui/columns.rs` tests, column UI tests, focused app tests |
| Change TUI attachment placeholder rendering | `src/task_render.rs`, `src/tui/ui/detail.rs` | `src/task_enrichment.rs`, `src/query/hydration.rs`, `src/tui/markdown.rs` for ordinary Markdown images | focused detail rendering tests, query enrichment tests, `cargo test tui::markdown --lib` |
| Change TUI terminal attachment previews | `src/tui/preview_controller.rs`, `src/tui/inline_images.rs`, `src/tui/app_lifecycle.rs` | `src/attachments/preview.rs`, `src/tui/ui/detail.rs`, inline image config | `cargo test tui::preview_controller --lib`, `cargo test tui::inline_images --lib`, focused lifecycle and detail rendering tests, `cargo check` |
| Add or change the TUI recent actions view | `src/query/recent_actions.rs`, `src/tui/ui/recent_actions.rs` | `src/change_log.rs` operation names, `src/tui/store.rs`, `src/tui/store/sidebar.rs`, view commands in `src/tui/event/catalog.rs` | `cargo check`, focused TUI store or app tests |
| Change TUI search flow | `src/tui/app_search.rs` | `src/tui/app_dispatch.rs`, `src/tui/overlay/`, `src/tui/ui/overlays/search.rs`, `src/query/` search helpers | `src/tui/app_tests.rs`, focused search and query tests |
| Add or change the TUI add-task composer | `src/tui/overlay/state.rs`, `src/tui/overlay/handlers.rs`, `src/tui/ui/overlays/add_task.rs` | `src/tui/app_authoring.rs`, `src/tui/app_dispatch.rs`, picker and label child controls, `src/tui/natural_add_runtime.rs` | `cargo test add_task` |
| Add or change TUI onboarding | `src/tui/app_onboarding.rs`, `src/tui/store/onboarding.rs` | timeline scheduling in `src/tui/app_lifecycle.rs`, splash rendering in `src/tui/ui/splash.rs`, `src/tui/overlay/`, `src/tui/ui/overlays/onboarding.rs`, welcome command in `src/tui/event/catalog.rs` | focused app, store, overlay, splash rendering, and welcome rendering tests |
| Add or change a TUI action | `src/tui/event/catalog.rs`, `src/tui/app_dispatch.rs`, focused `src/tui/app_*.rs` modules | flow helpers, overlays, store module, undo, `src/tui/natural_add_runtime.rs` for natural-add worker setup | `src/tui/app_tests.rs`, `src/tui/store/tests.rs`, overlay module tests |
| Change CLI or TUI update behavior | `src/update/`, `src/commands/self_update.rs`, `src/tui/app_update.rs` | `src/lib.rs`, `src/tui/app_lifecycle.rs`, `src/tui/overlay/`, `src/tui/ui/overlays/update.rs`, `src/tui/ui/header.rs`, release artifact names in `.github/workflows/release.yml` | updater unit tests, CLI surface checks, focused app, overlay, and header tests |
| Add or change TUI overlay behavior | `src/tui/overlay.rs`, `src/tui/overlay/`, `src/tui/app_overlay_submit.rs` | `OverlayRoute::descriptor`, input helpers, state builders, view projection, submit dispatch, module-local tests | `cargo test tui::overlay` |
| Add or change TUI overlay rendering | `src/tui/ui/overlays.rs`, `src/tui/ui/overlays/` | overlay view models, shared dialog helpers, input helpers, theme | overlay rendering tests in `src/tui/ui/overlays/tests.rs` |
| Change sync protocol or conflict handling | `src/sync/wire.rs`, `src/sync/apply/`, `src/sync/server.rs`, `src/sync/client.rs` | `src/mutation.rs`, `src/task_fields.rs`, `src/daemon.rs`, migrations if persisted | `tests/cli_sync*.rs`, `tests/cli_conflicts.rs`; focused bounded-sync checks include `cargo test --test cli_sync sync_server_returns_bounded_pull_pages`, `cargo test --test cli_sync sync_client_drains_paged_remote_changes`, `cargo test --test cli_sync sync_client_drains_paged_local_changes`, and `cargo test --test cli_sync wrong_response_protocol_version_is_rejected` |
| Add or change backup, export, or import commands | `src/cli.rs`, `src/lib.rs`, `src/commands/data_safety/`, `src/db.rs` | doctor output and integrity checks in `src/commands.rs` | `tests/cli_data_safety.rs`, `tests/cli_doctor.rs` |
| Change config, workspace, or project path routing | `src/config.rs`, `src/config_edit.rs`, `src/workspaces.rs`, `src/projects.rs` | config text writes, managed-entry edits, doctor, project commands, TUI workspace and project pickers | `tests/cli_config_daemon.rs`, `tests/cli_workspaces.rs`, `tests/cli_doctor.rs` |
| Change natural-language task intake or agent primer | `src/task_intake.rs`, `src/skill.md`, `src/commands/skill.rs` | config schema, `aven prime`, add-task flows, `src/tui/app_intake.rs` for TUI intake state and lifecycle, `src/tui/natural_add_runtime.rs` for TUI background worker setup | `tests/cli_task_intake.rs`, `tests/cli_skill.rs`, focused add-task tests |
| Change logging | `src/logging.rs` and call sites | safe field policy in guardrails | `tests/cli_logging.rs` |

## Common feature checklists

### Add or change attachment behavior

1. Put validation in `src/attachments/validation.rs` and byte persistence in `src/attachments/storage.rs`.
2. Route local attachment mutations through `src/operations/attachments.rs` so metadata rows, sidecar bytes, and attachment change rows stay aligned without changing task descriptions.
3. Keep CLI orchestration and metadata-only output in `src/commands/attachments.rs`.
4. Keep read-surface metadata loading in `src/task_enrichment.rs`, `src/query/`, and `src/task_render.rs`.
5. Keep backup, restore, export, import, and doctor attachment checks in `src/commands/data_safety/` and `src/commands/doctor.rs`.
6. Keep sync metadata validation, remote apply, blob endpoints, and client blob transfer aligned across `src/sync/wire.rs`, `src/sync/server.rs`, `src/sync/client.rs`, `src/sync/apply/`, and `src/daemon.rs`.
7. Keep TUI attachment sections metadata-backed through `src/task_enrichment.rs`, `src/tui/markdown.rs`, and `src/tui/ui/detail.rs`.
8. Update `src/skill.md` when command syntax, JSON availability, output fields, sync counts, or data-safety guidance changes.

### Add a CLI command

1. Add args and a `Commands` variant in `src/cli.rs`.
2. Classify the variant in the exhaustive `CliDispatch::from` match in `src/lib.rs` and add its command metadata in `src/command_metadata.rs`.
3. Add handling to the corresponding standalone, database, or TUI dispatch match in `src/lib.rs`.
4. Add command handling and output formatting to the focused command-family module under `src/commands/`, then export the entry point from `src/commands.rs`.
5. Put transactional business logic in `src/operations/`.
6. Add integration tests in `tests/`.

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
6. Use focused validation commands: `cargo test --test cli_sync sync_server_returns_bounded_pull_pages`, `cargo test --test cli_sync sync_client_drains_paged_remote_changes`, `cargo test --test cli_sync sync_client_drains_paged_local_changes`, `cargo test --test cli_sync wrong_response_protocol_version_is_rejected`, and `cargo test --test cli_daemon_sync daemon_syncs_large_backlog_across_budgeted_rounds`.

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

- `just check`: fast local read-only validation, equivalent to `just pre-commit`.
- `just test`: Rust test suite through `cargo nextest`, plus Rust doctests.
- `just check-full`: full local gate with tests, doctests, build, and SQLx validation.
- `just migration-new <lower_snake_name>`: create the next SQLx migration filename safely.
- `just sqlx-prepare`: regenerate SQLx offline query metadata after migrations or query shape changes.
- `just sqlx-check`: verify SQLx offline query metadata.
- `just run -- ...`: run the application.

The pre-commit hook runs formatting, static analysis, migration order checks, and clippy. During implementation, run the focused tests identified in the change-routing table. The pre-push hook runs the full local gate, including tests, doctests, build, and SQLx validation.
