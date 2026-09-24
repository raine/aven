# Architecture

`aven` is a local-first task manager implemented as a Cargo workspace. The `aven-core` crate owns domain and SQLite behavior, while the root `aven` package provides the CLI, Ratatui TUI, HTTP sync transport and server, daemon, configuration routing, and update delivery. This file is a sitemap for coding agents. For agent-facing command usage, also read `src/skill.md`.

The workspace contains only `aven` and `aven-core`. The consumer API uses
platform-neutral names so mobile adapters can share its reads and mutations.
Platform identity remains explicit data where it affects domain semantics, such
as `TaskSource::Ios` and `TaskSource::Android`. Mobile adapters and app build
tooling are maintained outside this workspace.

SQLite backend selection lives in the target-specific SQLx dependencies in
`crates/aven-core/Cargo.toml` and the root package's test dependencies. The shared
workspace dependency supplies runtime, macros, and migrations only. iOS device
and simulator targets (`target_os = "ios"`) select `sqlite-unbundled`; other
platforms retain the `sqlite` umbrella and its bundled engine. Consumer features
are additive: enabling `sqlite` elsewhere defeats the iOS system-link contract.
SQLx's unbundled macro route also requires host build-time bindings, so Apple
consumers must isolate host and target SDK/header/linker inputs in their tooling.

The iOS selection omits extension loading, deserialize, and unlock-notify.
Without unlock-notify, SQLx returns shared-cache lock errors instead of waiting
for unlock notifications; SQLite's busy timeout still handles ordinary busy
locks. Core file pools use WAL and five connections, and in-memory pools retain
one connection. Explicit shared-cache connection URLs need separate contention
validation. System-engine runtime compatibility and minimum-OS support require
Apple-side validation, not just bundled host tests.

## System map

| Layer | Owns | Start here | Rules |
| --- | --- | --- | --- |
| CLI entry and dispatch | argument parsing, command routing, config load, core database open, daemon wake dispatch, coding-agent skill installation | `src/main.rs`, `src/lib.rs`, `src/cli.rs`, `src/cli/`, `src/commands.rs`, `src/commands/` | `src/commands.rs` is the module and re-export facade. Command-family modules own command orchestration and command-local formatting. Domain reads and writes go through `aven_core::db::Database`. |
| Core facade | owned database handle and application-facing domain operations | `crates/aven-core/src/db.rs`, `crates/aven-core/src/operations/`, `crates/aven-core/src/query.rs` | `Database` owns a WAL-backed SQLx pool, concurrent reader acquisition, one process-serialized writer acquisition path, and complete transaction boundaries. Application code passes owned domain inputs and receives owned records or reports. |
| Core consumer API | workspace discovery, task and recurrence creation and updates, queue capture, state-checked queue quick mutations, task soft deletion and restoration, conditional undo, workspace-scoped project browsing and task search, recurrence lifecycle and reports, and conflict inspection and resolution | `crates/aven-core/src/api.rs`, `crates/aven-core/tests/consumer_api.rs` | `api::Store` wraps `Database` and exposes owned, platform-neutral consumer DTOs, typed error codes, recurrence-aware mutation routing, and reconcile-then-query reports. `TaskCapture::source` records the calling platform without splitting the operation by platform. The consumer API has no sync transport; encrypted sync lives in `sync::encrypted_tail` and the root host adapters. SQLx handles, transactions, internal operation types, query implementation, schedule math, sync JSON, configuration, transport, and rendering remain behind or outside the facade as appropriate. |
| Device invitation presentation | invitation-to-QR presentation state shared by CLI and TUI | `src/pairing.rs`, `src/sync/encrypted.rs`, `src/tui/app_pairing.rs`, `src/tui/ui/overlays/pairing.rs` | `PairingPresentation` renders an already encoded `aven://pair/v2/` device invitation into a display-safe server origin and `PairingQr`, whose compact half-block rows include the four-module quiet zone. `aven sync invite` prints the invitation text on standard output and the QR on an interactive standard error. The TUI `:add-device` command reuses `sync::encrypted::create_invitation` and `await_admission` through `InviteController`, shows the QR overlay, and keeps waiting after the overlay closes. Debug and error output omit QR rows and invitation text. |
| Write model | transactional task, recurrence aggregate, project, label, conflict, workspace, undo, and remote changes | `crates/aven-core/src/operations/`, `crates/aven-core/src/mutation.rs`, `crates/aven-core/src/task_fields.rs`, `crates/aven-core/src/undo.rs` | `Database` owns each production immediate mutation transaction, including no-undo operations, authoritative before-value reads required by read-modify-write inputs, related change-log and field-version writes, and an optional TUI undo entry. Label rename and deletion atomically update task and recurrence-template references, and their sync operations preserve those references across replicas. The shared task mutation implementation materializes full before-and-after reports for TUI undo callers and compact outcomes for no-undo update callers. Connection-level mutation helpers are crate-private transaction-local building blocks. Synced scalar task writes update tasks, `changes`, and `field_versions` together. Recurrence operations own series, occurrence, generated task, lifecycle, and task-status transaction boundaries. Conflict resolution emits the canonical synced operation. |
| Read model | task lists, task details, task search, project lists, sidebar counts, filters, sorting, refs, enrichment, recurrence series reports, and grouped recurrence history | `crates/aven-core/src/query.rs`, `crates/aven-core/src/query/`, `crates/aven-core/src/task_enrichment.rs`, `crates/aven-core/src/refs.rs`, `crates/aven-core/src/queue.rs` | Use the task-detail model for enriched single-task reads. Detail hydration includes a bounded, batch-loaded task activity projection derived from the same change-log vocabulary as Recent Actions. Base consumer task lists share flat SQL selection and batch recurrence grouping without task enrichment. Summary task-list reads omit note bodies, attachment metadata, metadata values, activity, and relationship detail, remain batch-oriented, and apply SQL limits before hydration when SQL owns the final ordering. Queue summaries add only classifier-derived typed reasons and date roles, ordered first-label summaries, epic identity, batch-counted live attachments, and a workspace unresolved-conflict aggregate. TUI task-list refreshes use summary hydration and upgrade selected detail tasks on demand. Search reads remain batch-oriented. Recurrence-aware reports reconcile a bounded candidate set before reading current projections. |
| Persistence | SQLite setup, embedded migrations, data safety, sync metadata, conflicts, durable local application state, and SQLx metadata | `crates/aven-core/src/db.rs`, `crates/aven-core/src/db/`, `crates/aven-core/src/local_state.rs`, `crates/aven-core/src/data_safety.rs`, `crates/aven-core/src/data_safety/`, `crates/aven-core/migrations/`, `crates/aven-core/.sqlx/` | Create migrations with `just migration-new <lower_snake_name>`. Core APIs own SQLx handles and transaction lifetimes. File databases use one WAL-configured pool with concurrent readers and a shared writer gate; in-memory databases retain one connection so their state remains coherent. Every mutation path acquires the writer gate before retaining `BEGIN IMMEDIATE` transaction ownership. Recurrence-bearing exports preserve aggregate rows and validate deterministic identities before replacement. Refresh SQLx metadata after query or schema changes. |
| Task metadata | workspace field definitions, task and recurrence-template values, field rename, metadata search and filters, sync aliases and conflicts, undo, and portable data | `crates/aven-core/src/metadata.rs`, `crates/aven-core/src/metadata/`, `crates/aven-core/src/sync/apply/metadata.rs`, `src/commands/metadata.rs`, `src/commands/tasks.rs` | Metadata fields have stable random IDs and canonical workspace-unique keys. Relations and conflict identities use field IDs so rename does not rewrite values. Recurrence template values copy into later materialized tasks. Full CLI, consumer, and TUI detail surfaces expose metadata; task summaries omit values. `src/tui/app_metadata.rs` coordinates the task field editor in `src/tui/overlay/metadata.rs` with captured task identity. Core metadata inputs can require an existing field identity and key within the write transaction. |
| Image attachments | content-addressed object storage, image validation and optimization, attachment metadata, lifecycle policy, leases, atomic task attachment mutations, attachment read models, terminal previews, and secure viewer exports | `crates/aven-core/src/attachments/`, `crates/aven-core/src/operations/attachments.rs`, `crates/aven-core/src/operations/tasks/`, `crates/aven-core/src/task_enrichment/`, `src/attachments/` | Core owns durable attachment state and object lifecycle. Root modules own filesystem input and output, terminal preview rendering, secure temporary exports, configuration, and transport. |
| Config and routing | config files, managed config text edits, path mappings, workspace resolution, and project inference | `src/config.rs`, `src/config/`, `src/config_edit.rs`, `src/workspaces.rs`, `src/projects.rs`, `src/routing.rs`, `src/operations/projects.rs` | CLI workspace selection and project inference share `InvocationRouting`, borrowing loaded config and caching canonical cwd and Git root on demand. Application code passes resolved routing inputs to the core. Managed entry text surgery belongs in `src/config_edit.rs`. |
| Sync and daemon | Reqwest and Axum transport, protected local package keys, shared-state capture and installation, cross-process host coordination, protocol validation and persistence, observational status reports, launchd service management, wake policy and loop | `src/protected_local_keys.rs`, `src/sync.rs`, `src/sync/`, `src/status.rs`, `src/daemon.rs`, `src/daemon/`, `crates/aven-core/src/sync/` | `src/protected_local_keys.rs` is the host boundary for the local capture package's vault, generation, and key. It scopes authority to a canonical database installation, uses a non-synchronizing login Keychain item on macOS or an owner-only state file on Linux, and persists authority before package creation. Root sync modules own Reqwest, Axum, host scheduling, and typed privacy-safe status projection. Core has no unencrypted sync session or server; its request/response envelope and page persistence exist only behind the `test-support` feature as an in-process simulator that core fixtures use to produce accepted history. `crates/aven-core/src/sync/shared_state.rs` captures materialized shared state plus retained history at one SQLite boundary, persists the single never-dispatched local capture with exact history and durable image ownership, and atomically installs shared state into a fresh database. `crates/aven-core/src/sync/shared_state/package.rs` owns the single durable never-dispatched upload package. Its `publication.rs` codec (exported as `sync::bootstrap_format`) encodes the domain, descriptor, three catalogs and encrypted manifest; the capture candidate ID is the bootstrap artifact ID. SQLite atomically freezes the exact upload components, the descriptor and catalogs plus encrypted records keyed by component, image object and index, with a journal-owned descriptor commitment; no other package metadata is stored. Reopen authenticates the frozen representation against the immutable capture without rereading live domain or image files. Missing/corrupt frozen data fails closed; local-only packages without publication components require explicit cancellation/recapture, never rewriting under the same ID. `EncryptedLocalSharedStatePackage::upload_package` returns exact components for keyless validation or context-bound client authentication. The package includes selected current and extra image bytes; validated unavailable metadata has no invented object mapping. Image objects use random identities and generation-derived keys. Private plaintext-hash mappings remain in the capture and encrypted domain records, not public descriptors. Core accepts protected key material and never persists it. The host accepts an explicit membership predecessor as context only, not authorization. Local package context alone is not claim authority. `sync/seed_claim.rs` owns the fixed signed genesis, HPKE self-package validation and one-vault SQLite admission; `src/protected_local_keys/seed.rs` persists installation-bound seed keys, bearer and exact genesis before use. `sync/bootstrap_staging.rs` authenticates current supported membership, stages bounded exact bytes, and atomically publishes the fixed signed genesis successor with complete catalogs, image ownership and allocator=N. Immutable outcomes are separate from current-head authority. `shared_state/adoption.rs` and `src/protected_local_keys/adoption.rs` own original-seed adoption with SQLite-first cancellation exclusion, separately protected source/intent records, history-only remapping and post-commit pin cleanup. `db/installation.rs` owns a denial-only replacement marker and short-lived path lock outside replaceable SQLite. `src/seed_bootstrap_http.rs` provides isolated seed HTTP dispatch through original-seed adoption. `seed_claim/membership`, `src/protected_local_keys/{peer,membership}.rs` and `src/peer_enrollment_http.rs` extend the signed chain through repeatable same-generation device admission and independent protected credentials. Authorized published reads and protected fresh-peer installation reuse that authority, the exact package codecs and the shared-state transaction. The internal ordinary-task tail lives in `sync/encrypted_tail` and `src/encrypted_tail_http.rs`. There is no general membership engine. `src/sync/encrypted.rs` is the provisional CLI over these adapters (see Encrypted sync CLI); protocol/security review, interoperability, production resource limits and platform/power-loss durability remain gates. Root host operations hold the file lock across a complete interactive drain or one daemon round. |
| TUI app | launch-intent resolution, event loop, actions, overlays, store, rendering, natural add runtime, and platform helpers | `src/tui/store/launch.rs`, `src/tui/` | `TuiStore` owns a cloned core `Database` handle. The launch resolver turns CLI targets into one view state before app construction. UI modules render view models and never access SQLx or SQLite directly. |
| Update delivery | cached GitHub release discovery, semantic version comparison, install ownership classification, verified direct replacement | `src/update.rs`, `src/update/` | Background checks are fail-silent and rate-limited. CLI and TUI flows own presentation and confirmation behavior. |
| Shared domain types | validated IDs, status and priority values, task and project records, query DTOs, sync DTOs | `crates/aven-core/src/ids.rs`, `crates/aven-core/src/choices.rs`, `crates/aven-core/src/types.rs`, `crates/aven-core/src/query/types.rs`, `crates/aven-core/src/sync/wire.rs` and `crates/aven-core/src/sync/wire/` | Domain types are independent of CLI arguments, TUI state, and config files. Application-only rendering and input parsing remain under root `src/`. |
| Recurrence domain | validated series IDs, anchored daily, weekly, monthly, and yearly interval rules, direct slot navigation, Monday-anchored weekly expansion, month-end and leap-day clamping, series-zone slot timing, DST resolution, deterministic occurrence identity, local aggregate operations, recurrence hydration, series reports, and grouped history | `crates/aven-core/src/recurrence.rs`, `crates/aven-core/src/recurrence/`, `crates/aven-core/src/operations/recurrence/`, `crates/aven-core/src/query/recurrence.rs` | Schedule math and occurrence identity stay pure and device-independent. Core recurrence operations atomically own template snapshots, projection reconciliation, outcomes, lifecycle, stable refs, and eligible immediate undo. Local scalar and structural task mutations use the typed gate in `operations/recurrence/resolution.rs` inside the owning transaction. The operation supplies the timestamp for reconciliation and outcome resolution, and scalar callers handle `Proceed`, `NoChange`, and `Handled` before generic writes. Core reports batch-hydrate summaries, group historical presentation, and keep direct detail access separate from ordinary task visibility. |
| Tests and tooling | core extraction tests, CLI integration tests, feature-oriented TUI app, store, and overlay test modules, SQL index checks, just tasks | `crates/aven-core/tests/`, `tests/`, `tests/common/`, `src/tui/app_tests/`, `src/tui/store/tests/`, `src/tui/ui/overlays/tests/`, `justfile` | Put unit tests in the module that owns the behavior, as an inline `#[cfg(test)] mod tests` or a sibling `*_tests.rs` when the body is large. A facade's own tests may sit in `<facade>/tests.rs` when they exercise the facade rather than its children, and feature-oriented trees such as `src/tui/app_tests/`, `src/tui/store/tests/`, and `src/tui/ui/task_list/tests/` deliberately group tests by feature across one owning module. What does not belong is a single module holding tests for several sibling modules, which leaves the tests further from the code than the split they follow. Shared fixtures belong in a `test_support.rs` beside them. Run focused tests while developing; the pre-commit hook runs the fast format, lint, and migration gate, and the pre-push hook runs `just check-full`. |

### Facade and responsibility modules

Several subsystems are a thin `foo.rs` facade over a `foo/` directory. The facade
owns module wiring, subsystem-wide types, and the re-exports that define the
surface; its children own the behavior. Start at the child that owns the
responsibility, not at the facade. Facades carry no blanket
`allow(unused_imports)`, so a re-export no caller uses is a warning rather than
silent accumulation. Tests live with the module they exercise, and shared
fixtures live in a `test_support.rs` beside them.

| Facade | Children |
| --- | --- |
| `crates/aven-core/src/db.rs` pool, migrations, meta | `backup.rs` in-process backup and restore, `changes.rs` change-log inserts, `field_versions.rs` field and conflict identity, `inspection.rs` disposable snapshots, `rows.rs` row-to-domain mapping |
| `crates/aven-core/src/data_safety.rs` export and import entry points | `scan.rs` table reads, `export_types.rs` portable schema, `validation.rs` and `validation/recurrence.rs` export-payload rules, `integrity.rs` with `integrity/database.rs`, `integrity/attachments.rs`, and `integrity/recurrence.rs` live-database checks, `import.rs` replacement, `archive.rs` and `tables.rs` archive mechanics |
| `crates/aven-core/src/metadata.rs` shared limits and types | `fields.rs` field identity and rename, `values.rs` task and recurrence values, `validation.rs` update and result limits |
| `crates/aven-core/src/operations/tasks.rs` drafts and options | `creation.rs`, `attachment_creation.rs`, `mutation.rs`, `notes.rs`, `consumer.rs` queue quick mutations, soft deletion and restoration, and their conditional undo |
| `crates/aven-core/src/operations/recurrence.rs` shared template fields and reconcile policy | `template.rs` series creation and template edits, `projection.rs` reconciliation and materialization, `lifecycle.rs` pause, resume, and stop, `resolution.rs` occurrence resolution, its task-mutation gate, and undo |
| `crates/aven-core/src/operations/conflicts.rs` | `reads.rs` conflict listing and variants, `task.rs`, `metadata.rs`, and `recurrence.rs` per-domain resolution |
| `crates/aven-core/src/task_enrichment.rs` batch hydration entry points | `attachments.rs`, `dependencies.rs`, `epics.rs`, `notes.rs` |
| `crates/aven-core/src/attachments/lifecycle.rs` policy constants and clock | `filesystem.rs` object moves and bounded directory scans, `leases.rs`, `liveness.rs`, `quota.rs`, `maintenance.rs` prune and reconcile, `report.rs` |
| `crates/aven-core/src/sync/wire.rs` protocol constants, dispatch, attachment payloads | `envelope.rs` request and response bounds, `changes.rs` per-operation payload rules, `recurrence.rs` recurrence payload rules |
| `crates/aven-core/src/sync/persistence.rs` local sync status | `changes.rs` change identity, server-sequence updates and epic reconciliation, `blobs.rs` attachment liveness hashes, `parent_liveness.rs` accepted-order retention state, `status.rs` local counters; `client.rs` and `server.rs` are the `test-support` in-process page simulator |
| `crates/aven-core/src/sync/shared_state.rs` capture and install | `package.rs` durable freeze/load, protected-key inputs and shared chunk crypto; `package/publication.rs` exact publication codec and validation; `publication/domain.rs`, `catalog.rs`, `projection.rs` and `codec.rs` typed domain, committed catalogs, public/private agreement and bounded framing; `package/durable_tests.rs` restart, rollback, format refusal and process-exit evidence |
| `crates/aven-core/src/sync/shared_state/adoption.rs` original seed association | Exact source/history validation, preparing/sealed/adopted SQLite intent and collision-safe remapping; `src/protected_local_keys/adoption.rs` separately protected source and exact intent; `db/installation.rs` denial-only path marker and replacement interlock. Transport dispatch uses the sealed protected intent through `src/seed_bootstrap_http.rs`; no restore authorization. |
| `crates/aven-core/src/sync/seed_claim.rs` fixed genesis and private authority | `seed_claim/codec.rs` immutable genesis bytes; `seed_claim/publication.rs` fixed signed successor and expected-descriptor verification; `seed_claim/persistence.rs` transactional one-vault admission and nonsecret local loss-detection pin; `src/protected_local_keys/seed.rs` host seed storage and package-context binding |
| `crates/aven-core/src/sync/bootstrap_staging.rs` authenticated declaration, chunk, status, resume and cancellation contracts | `bootstrap_staging/persistence.rs` serialized SQLite staging and reclamation; `bootstrap_staging/persistence/publication.rs` current-head authorization, completeness, atomic READY and ownership transfer; `shared_state/package/publication/staging.rs` transaction-local views of the existing publication codec, not another format |
| `src/cli.rs` `Cli` and `Commands` | argument families in `tasks.rs`, `relationships.rs`, `recurrence.rs`, `sync.rs`, `data_safety.rs`, `administration.rs`, `tui.rs`; `help.rs` owns styles, sections, and row rendering |
| `src/config.rs` `AppConfig` and serialization | `paths.rs` path and server resolution, `tui.rs` lanes, table columns, and sidebar views, `custom_commands.rs` custom command config and validation |
| `src/task_render.rs` shared task output entry points | `text.rs`, `json.rs`, `markdown.rs`, `attachments.rs` |
| `src/commands/doctor.rs` staged run order | `bootstrap.rs` tolerant config and path resolution, `checks.rs` database and daemon sections, `report.rs` section and row model, `render.rs` |
| `src/tui/platform.rs` | `terminal.rs` suspension and keyboard enhancement, `clipboard.rs`, `editor.rs`, `viewer.rs` image viewer and browser launchers, `gist.rs` |
| `src/tui/overlay/state.rs` overlay state wiring | `authoring.rs` composer fields and recurrence preview, `editors.rs` text and date editors, `command_search.rs` |
| `tests/common/mod.rs` | `database.rs` fixtures and SQL helpers, `process.rs` child process and log waiting, `server.rs` test server startup |


## Website

`docs/` builds the public site with Astro. `docs/src/pages/index.astro` and
`docs/src/styles/landing.css` own the standalone landing page. The privacy policy
at `docs/src/pages/privacy.astro` shares the landing styles, with reading-layout
styles in `docs/src/styles/privacy.css`. Starlight owns the
documentation routes in `docs/src/content/docs/`, starting with `overview.md` and
`getting-started.md`; its navigation is configured in `docs/astro.config.mjs`.
Product screenshots and locally served, licensed fonts live in `docs/public/`.
Validate website changes with `cd docs && bun run build` and inspect layout and
interactions in a browser. Landing styles must stay independent of the docs theme.
Use standalone SVG components for landing-page icons. Importing the Starlight
components barrel can pull its global reset into the landing page and override
body typography and background colors.

## Runtime flows

### CLI command flow

1. `src/main.rs` starts Tokio and calls `aven::run_cli()`.
2. `src/cli.rs` parses `Cli` and `Commands`.
3. `src/lib.rs` classifies every parsed command into the explicit standalone, database, or TUI dispatch class through an exhaustive `Commands` match. Database and TUI paths open an opaque `aven_core::db::Database`, resolve application routing, then call core-owned reads and mutations. Doctor is standalone and owns staged, tolerant config and database inspection under `src/commands/doctor/`, with `bootstrap.rs` resolving config and database paths tolerantly and `checks.rs` owning the database and daemon sections.
4. `aven_core::db::Database::inspect` copies an existing database and available WAL and shared-memory sidecars into temporary isolated storage, then opens that snapshot for diagnostic queries. Doctor uses the resulting database only when its schema is current and supported. Inspection never invokes normal database initialization or migration behavior against user state. Checks that reconcile derived state operate only on the disposable snapshot.
5. Mutating commands call core operations through `Database`, then dispatch through the daemon wake-if-enabled policy.
6. `src/commands/recurrence.rs` parses the documented fixed rule grammar and recurrence-qualified calendar flags into validated core values. It renders versioned series reports and resolves both series and occurrence refs through core before mutations.

### Attachment lifecycle

1. `src/commands/attachments.rs` reads and writes user-selected files, resolves task references, and formats metadata-only output.
2. `crates/aven-core/src/operations/attachments.rs` validates decoded images, stores content-addressed bytes, owns attachment metadata and lifecycle leases, and records synced attachment changes. TUI paste reserves attachment identity and ordering before background preparation, while synced metadata and its SHA-256 enter SQLite only after final canonical bytes are ready. Task creation stages distinct objects before one immediate transaction commits task rows, inventory, attachment metadata, field versions, and change rows.
3. `crates/aven-core/src/sync/` plans bounded blob work, owns transfer leases and request state, applies metadata and bytes transactionally, and leaves Reqwest and Axum adaptation under `src/sync/`. Missing-object summaries use SQL aggregates, database-known missing objects are hydrated in bounded pages, and attachment maintenance reconciles inventory entries whose object files disappeared.
4. `crates/aven-core/src/data_safety.rs` and its children own backup, restore, import, integrity, and attachment consistency mechanics. Root command modules own filesystem selection and presentation.
5. `src/tui/ui/detail.rs` is the detail-rendering facade. Its `detail/` children own cached document geometry and interaction (`document.rs`), section assembly (`body.rs`), text wrapping and selection (`text.rs`), relationship rows (`relationships.rs`), attachment rows and previews (`attachments.rs`), and metadata (`metadata.rs`). The facade renders committed attachment metadata plus app-owned pending and failed attachment state. `src/tui/attachment_controller.rs` owns bounded asynchronous paste preparation, reserved identity and ordering, completion state, and shutdown cleanup. `src/tui/preview_controller.rs` owns bounded thumbnail work and payload caching. Separate blocking pools prevent preview generation from holding attachment preparation capacity.
6. `src/attachments/export.rs` creates leased temporary copies with media-appropriate extensions for `src/tui/platform/viewer.rs` to open in the OS image viewer. `src/attachments/save.rs` creates user-selected durable copies under read leases and refuses to replace existing destinations.

### Recurrence report flow

1. Core task list, detail, queue, search, sidebar, recent-action, and series report methods select at most 256 active recurrence candidates for one workspace.
2. Each candidate reconciles in its own immediate transaction. A boundary crossing can archive one superseded projection and materialize one current projection through one atomic projection operation before the report reads.
3. Lifecycle conflicts return existing state without projection writes. An incomplete reconciliation result makes the candidate bound explicit.
4. Ordinary task paths exclude paused projected tasks and archived projections. Stopped final open tasks remain visible, while direct task detail and series history retain archived and paused task access.
5. Task enrichment loads recurrence summaries by task ID chunks. Grouped Done, search, sidebar, and recent-action reports use series identity, while recurrence history merges task-backed outcomes, archived misses, derived misses, and pause intervals. Taskless historical slots have no occurrence row and remain derived misses.

### TUI flow

#### Launch and input

1. `src/tui/store/launch.rs` resolves independent CLI query and layout arguments into one complete `TaskViewState` and startup action before `src/tui/mod.rs` constructs `App`. Direct task targets use workspace-scoped Search state so hidden tasks remain addressable.
2. Pure routers in `src/tui/input/key.rs` and `src/tui/input/mouse.rs` translate terminal events into semantic actions or feature-owned input events. `src/tui/overlay/mouse.rs` owns overlay hit testing and state transitions behind typed outcomes that `src/tui/app_overlay_input.rs` applies. `src/tui/app_dispatch.rs` owns the thin top-level key and paste entry points, while `src/tui/app_mouse.rs` coordinates mouse routing, `src/tui/app_overlay_input.rs` coordinates overlays, and `src/tui/app_detail_input.rs` coordinates detail interaction. Capturing overlays receive their focused events after top-level routing.
3. The exhaustive command table in `src/tui/input/action.rs` executes `Action` values and delegates feature transitions and effects to focused `src/tui/app_*.rs` coordinators. Captured command resolution and target-bearing execution live in `src/tui/app_commands.rs`; relationship target resolution remains in `src/tui/app_relationships.rs`.

#### Command catalog and activation

- `src/tui/event/catalog.rs` declares built-in commands. Each action projects one target policy and surface effect from `src/tui/event/action.rs`; the same catalog drives command-panel membership, contextual ranking, shortcut routing, help, and prefix hints. The target-free, keyless `:pair-mobile` command is available from list command panels. Typed detail searches report that it is available only in the task list. Activation resolves the server from the startup `AVEN_SYNC_SERVER` snapshot then `sync.server_url`, uses the trimmed shared token, and replaces the panel with the secret-safe **Pair mobile device** modal.
- Opening the command panel captures one non-optional `CommandSessionSnapshot` containing workspace, surface, task and mark identities, focused detail identity, sidebar target, and recurrence target. `CommandState` shares the catalog through `Arc`, queries it only through the session-aware pipeline, and caches candidates as stable catalog indices for rendering, mouse input, keyboard navigation, completion, and activation.
- Activation revalidates the selected catalog identity, resolves one typed `ResolvedCommandTarget` from the captured session, batch-hydrates missing tasks, and calls explicit target-bearing feature entry points. Confirmation, picker, and search intents own that identity through chained interactions. Command-panel activation has no cursor or focus fallback. Direct shortcuts capture a session and enter the same resolution path.

#### Application and navigation state

- `App` composes flow controllers, cohesive surfaces, and simple application scalars. `ListSurface` owns browse focus, widget selection, visible marks, sidebar state, identity-bearing navigation history, and list click recognition. Its sidebar projection applies section collapse to the configured store rows, restores selection by typed target, and supplies the same visible rows to rendering, keyboard navigation, and mouse hit testing. `src/tui/store/sidebar.rs` applies the ordered local `tui.sidebar.views` setting; view commands remain independent of sidebar visibility. Navigation entries pair `TaskViewState` and table offset with a `MainRowAnchor` captured from the live projection before a query refresh. `DetailSession` owns detail activity, interaction state, and linked-detail history. `InlineImageSurface` owns terminal image placement, cleanup, and external-viewer retention.
- `TaskViewState` owns task-list scope, query, layout, filters, ordering, direction, and recurring-series lifecycle or text filters. `TaskQuery` defines the selected task set, while `TaskLayout` selects list or columns presentation for compatible queries. `MainRowIdentity` distinguishes task IDs, recurrence-series IDs, and Recent Actions change IDs. `SelectionRestore` makes default, identity-only, identity-plus-position anchor, and index-only refresh behavior explicit. Compatible query transitions use anchors, resolve identity against the complete replacement projection, and fall back through surface-aware flat, column, or Epic coordinates. Row-domain and workspace transitions initialize or target their destination explicitly. `src/tui/columns.rs` partitions the ordered query result into configured columns and derives column navigation while retaining global task indexes. `TuiStore` retains pure task-row and column-lane indexes for the published task/view state, sharing them across rendering, input, and navigation while terminal geometry remains frame-local. `TaskSelection` captures stable existing-task targets for edit flows, resolving visible marks before the selected row in list views and targeting only the displayed task in detail view. The Recurring view renders series rows as the main list and loads one series detail projection for navigation into its applicable occurrence.
- Linked detail navigation loads exact workspace-scoped tasks through `TuiStore`. `DetailSnapshot` provides linked-history and exact-task restoration. `DetailSession` retains the source task-list order and selection anchor so sibling navigation remains relative to that list while linked tasks are outside its filters. Recent Actions task entry stores its source change identity and list position in `ListSurface`, so closing detail restores the scoped action list and stable row when it remains available. Modal overlays layer over an active detail session without owning parallel return flags.
- `SearchController`, `IntakeController`, `AttachmentController`, `PreviewController`, `GistController`, and `UpdateController` own their asynchronous work and lifecycle transitions. Attachment preparation and preview generation use separate bounded blocking pools.

#### Authoring flows

- `src/tui/app_authoring.rs` coordinates task creation and natural-language intake. `src/task_intake.rs` validates model JSON into a typed result containing an ordinary `TaskDraft` and an optional core-validated recurrence schedule. CLI and TUI consumers route recurrence through existing series authoring and creation paths. `AuthoringState` owns the active add-task draft together with an exhaustive origin: standalone creation or epic-child creation carrying its epic and search return state. Submission, retry, cancellation, discard confirmation, attachment preparation, and natural-add completion read that single flow.
- `src/tui/overlay/text_buffer.rs` owns intent-free editable text, byte cursor, baseline, and editing operations. Line-local key operations over a string and byte cursor live in `src/tui/overlay/text_input.rs` and are shared by `LineEdit` and `TextBuffer`; `TextBuffer` owns vertical movement, newline splits, cross-line merges, and word-deletion boundary handling. Standalone multiline state composes this buffer with flow intent, chrome, and discard mode, and normalizes pasted newlines in its adapter. Metadata composes the buffer directly and inserts opaque text exactly; its single-line editing and multiline preview rules stay in `src/tui/overlay/metadata.rs`.
- The add-task overlay owns visible editor state, focus, validation, pending-image presentation, natural schedule input with an expandable structured editor, core-validated recurrence preview, help, and child controls. Draft attachments remain in `AuthoringState` across nested controls and retries. Child controls preserve text and cursor state. Recurrence lifecycle and history flows live in `src/tui/app_recurrence.rs`, while `src/tui/store/recurrence.rs` adapts them to core aggregate operations and reports.
- `IntakeController` owns natural-add configuration, add-task-only mode, worker state, cancellation, and polling. `src/tui/natural_add_runtime.rs` owns worker command construction and process setup.
- New task creation uses the current list index only as a selection anchor. `TaskSelection` remains reserved for mutations of captured existing tasks.

#### Persistence and refresh

- Store modules call owned methods on `aven_core::db::Database`. `TuiStore` owns the database handle, application config, and refresh-independent state. Its `TuiProjection` owns the complete refresh result exposed to app and rendering code. `TuiStore` lazily retains the pure task-row and column-lane indexes derived from that result, invalidating them through the mutable projection and column-configuration boundaries. Date-relative Upcoming row indexes are rebuilt from the render-time clock while stable views retain their indexes. Atomic refresh construction copies only the fields represented by `RefreshRetainedState`, while `TuiProjection` remains non-cloneable in production so hydrated projection collections cannot enter the replacement shell. Each task mutation family has one batch-capable policy method in `src/tui/store/task_commands.rs` for one or many `TaskSelection` targets.
- Persisted TUI mutations request undo from the owning core operation. The core transaction loads authoritative values, applies all targets, derives undo, writes one grouped entry when needed, and commits once. Individual note edits and deletes use the hydrated note ID as stable identity, emit note-specific sync changes, and retain that identity through edit and undo.
- Structured mutation reports drive messages and selection restoration. A TUI refresh reconciles recurrence once, then builds a complete replacement projection from current-projection reads, using summary hydration for task-list rows. The selected task is upgraded to detail hydration before detail rendering, and a restored task identity is upgraded before replacement publication. Detail hydration reports ready resident tasks separately from requested IDs absent from the projection and resident summaries unresolved by the detail read. Callers remove, close, refresh, or rebind stale detail state, and only `TaskItemHydration::Detail` is canonical detail. Refresh resolves its explicit selection policy against the final effective scope and publishes it only after every query succeeds. `ScopeRefreshResult.selected` is the application layer's authoritative widget row. A committed-refresh error closes completed edit flows so stale input cannot duplicate a mutation.

#### Rendering and lifecycle

- `src/tui/ui.rs` and `src/tui/ui/` render application state without database or filesystem access. `src/tui/ui/empty_state.rs` owns shared list-empty semantics, catalog-backed action hints, and responsive rendering through surface-specific classifiers. `src/tui/ui/task_list.rs` is the task-list facade and orchestration boundary. Its `table.rs` owns render models and table painting, `cells.rs` owns row cell construction and formatting, `sizing.rs` owns content-sensitive constraints and visible-task collection, `layout.rs` owns semantic table and preview geometry, `hit_test.rs` owns task and status hit testing, `view_model.rs` owns logical rows and scrolling, and `preview.rs` owns selected-task preview rendering. These modules consume the retained row index from `TuiStore`, and task-list tests are grouped under `src/tui/ui/task_list/tests.rs` and its focused submodules. Task detail renders its bounded activity projection and identifies the event that establishes queue idle time; deferred tasks use their availability timestamp as the idle basis.
- `DetailDocument` in `src/tui/ui/detail/document.rs` retains revision- and width-keyed, scroll-independent body geometry shared by rendering and interaction, including wrapping, layout, focus targets for note identities and relationships, selection mapping, scroll bounds, hit testing, and image placement. The `src/tui/ui/detail.rs` facade reexports the document API. Each frame projects and styles only visible rows, and `WidgetState` retains the document for stable frame queries.
- `src/tui/app_lifecycle.rs` polls background completion, including application-owned custom command processes, hydrates selected task detail before detail frames, coordinates image emission after frame draws, runs terminal custom commands while the TUI loop is suspended, and performs orderly shutdown. `src/tui/app_custom_commands.rs` applies successful custom-command refresh and shutdown policies through the application lifecycle after process completion and terminal restoration, using the committed-projection refresh path before any refresh-dependent shutdown. `src/tui/custom_command.rs` purely plans expanded programs, static argv and environment, effective working directories and deadlines, and the versioned metadata-only JSON contract from application-supplied invocation paths. `src/tui/custom_command_runtime.rs` applies planned noninteractive process values and owns invocation identity and phase, direct process creation, concurrent input and bounded output I/O, whole-operation deadlines, asynchronous completion, and lifecycle cleanup. Waiting commands use application-owned cancellation and Unix process-group termination, while successful background input handoffs detach from application shutdown. `src/tui/terminal_command.rs` owns protected context-file delivery, inherited terminal process execution, foreground terminal process-group control on Unix, configured timeout cleanup through the shared process supervisor, and restoration-before-policy ordering. `src/tui/platform.rs` owns the RAII terminal suspension boundary shared by terminal commands and external editors, including raw mode, alternate screen, cursor, mouse capture, bracketed paste, and keyboard enhancements. Application terminal preparation reconciles inline-image state and forces a complete redraw after restoration. Attachment detail sections combine committed metadata with app-owned pending and failed preparation state.
- `src/tui/app_update.rs` coordinates update discovery and a unified Software Update overlay that presents target-release notes, install metadata, actions, progress, and outcomes in one fixed frame. `src/update/` owns release discovery, verification, install classification, and executable replacement. `src/tui/changelog.rs` fetches and caches canonical GitHub `CHANGELOG.md` content shared by the update review and historical changelog reader.

### Original-seed adoption and replacement exclusion

The explicit host sequence is `prepare_seed_claim`, `prepare_seed_source`, local
capture, `package_seed_capture`, then `prepare_seed_adoption_intent`. Source
preparation opts the installation into an irreversible denial-only replacement
fence before creating its separate protected incarnation. Existing captures
without that binding require explicit never-dispatched cancellation/recapture.
`aven sync setup` drives this sequence through `src/sync/encrypted.rs`.
`seed_bootstrap_http::Client::resume` prepares or resumes protected intent before
any staging or publication request.

`shared_state/adoption.rs` commits exact signed intent in SQLite before the host
persists it outside the task database. Preparing intent already prohibits local
cancellation, including core callers without a host lock. A capture-owned bit
and deletion trigger preserve that refusal if its intent row is lost. Protected readback and
source/generation CAS seal the same bytes. Partial setup resumes the same intent;
missing established authority never authorizes cancellation or regeneration.
The original seed key encoding is unchanged.

`adopt_seed_publication` authenticates the real publication outcome against pinned
genesis and the frozen descriptor. Under one immediate transaction it verifies
source identity and captured history, clears only captured ranks, assigns the
complete dense prefix, then reconciles epic membership. Current task state,
field versions/conflicts, recurrence metadata and later pending changes are not
reinstalled or replayed from the snapshot. Existing order-dependent projections
retain their relative history order. The adopted receipt, stream association and
cursor commit together. Retry validates the durable receipt without resetting
later progress, even after separate retry-safe capture/package/pin cleanup.

`db/installation.rs::InstallationGuard` resolves canonical parent plus basename,
including a missing final database path, and rejects final symlinks/hard-link
ambiguity. Its persistent marker is refusal only, never source authority. Core
physical SQLite and archive restore hold the same short-lived exclusive lock
across replacement. Ordinary plaintext operations and backups use shared locks,
so overlapping requests retain SQLite writer serialization instead of spuriously
contending on installation exclusion. Acquisition is nonblocking; shared holders
cannot establish the denial marker. Import and plaintext metadata/blob entry points reject selected
installations, including after capture cleanup or loss of SQLite source metadata.
This is not a database-lifetime lock; raw live replacement is unsupported. An
early opted-in setup failure can leave replacement fenced with no reset API.

Bound installations cannot use ordinary SQLite/archive backup or destructive
import/restore. Readable export adds `e2ee_data_only` and excludes the old plaintext
association; ordinary import refuses this marker. It prevents accidental downgrade,
not malicious export editing. Protected source and intent are never exported.
Standalone unbound plaintext workflows retain their ordinary data semantics.
Plaintext requests fail closed on bound installations, including retries from old sessions.
The isolated encrypted tail requires independent protected readiness and receipt checks.

Focused tests: `cargo test --lib 'protected_local_keys::adoption::tests::'`,
`cargo test -p aven-core --lib 'sync::shared_state::adoption::tests::'`, and
`cargo test -p aven-core --lib 'db::installation::tests::'`. Host tests use the
isolated file backend, real signing/staging/publication, process exits at committed
boundaries, SQL faults, and deterministic restore/source interleavings. They do
not prove Keychain or power-loss durability, transport, joining or restore/re-pair.

### Published snapshot retrieval and fresh-peer installation

`src/peer_enrollment_http.rs::Client::install` reloads the independent peer's
protected verified enrollment, pinned descriptor and generation key. Each published
record read carries vault, genesis, device, credential version, expected current
head and descriptor commitment. `shared_state/peer_install.rs` uses the same
chain-derived membership resolver as enrollment. Reads cannot see unpublished
staging or quarantine. Image reads copy available SQLite-owned bytes inside the
authorization transaction, so concurrent reclamation cannot invalidate that owned
response. They create no leases, bootstrap pins or replacement image bytes.

Downloads are bounded, sequential and memory-only. Restart discards partial bytes
and redownloads the same protected publication, never a server-selected candidate.
`package/publication/download.rs` derives recipes from the existing descriptor and
committed catalog validators. Its metadata-only join input authenticates the
same domain, manifest and private/public mapping agreement without requiring
image records. Complete seed publication and adoption still require all selected
current and extra image bytes; neither uses a missing-image bypass. A published
image catalog remains authenticated mapping history even after legitimate pruning.

The fresh-target transaction rechecks enrollment identity and emptiness, then uses
the same shared-state import allowlist as plain installation without relaxing its
bound-target refusal. It installs materialized state and dense retained history,
not replayed operations. Image mappings, unavailable inventory, the initial catch-up
marker and stream/prefix/checkpoint receipt commit together. Join installation has
no filesystem side effects and does not take a blob directory. Explicitly absent
bootstrap mappings remain distinct from mapped images whose bytes are unavailable.
Image demand is computed after the installation's initial tail watermark is reached;
ordinary transfer then validates bytes before making local inventory available.

The host records completion separately in protected storage after the DB commit.
An exact receipt retry validates protected authority and association locally,
without contacting the server or resetting later edits/cursor progress. Lost or
mismatched receipts are never repaired by reinstalling. A surviving protected
completion with a missing receipt refuses. SQLite receipts and enrollment mirrors
are not membership authority. Selected-installation replacement/backup exclusions
and unresolved-disclosure fences remain intact. Installation alone is not dispatch
authority: encrypted rounds reload protected readiness and verify current server
authorization. `aven sync join` drives request, completion and installation.

Focused evidence: `cargo test --lib 'peer_enrollment_http::tests::install::'`,
shared-state codec/install/adoption tests and plaintext attachment lifecycle tests.
The loopback fixture includes synthetic retained conflicts, not encrypted conflict
generation. Subprocess exits exercise metadata download/import, pre-commit and
post-commit boundaries, plus initial tail page rollback and committed restart.
These are process-restart tests, not power-loss, mobile resource or independent
interoperability evidence.

### Internal encrypted ordinary-task rounds

`src/encrypted_tail_http.rs` exposes an isolated bounded POST router and one-round
client, never the legacy shared token or a plaintext fallback. Core
`sync/encrypted_tail/{codec,domain,server,client}.rs` owns operation framing, strict
JSON/compatibility and canonical comparison, immutable ciphertext admission,
frozen outbox/acceptance evidence and verified ordered apply. The protected host
holds installation and key-store exclusion across the round, verifies both its
own enrollment and actual seed adoption or peer installation, and supplies keys
without persisting them in SQLite. Every server transaction uses the existing
chain resolver with the exact expected head, stream and published descriptor.

Task create/edit/delete/restore, notes, labels, metadata, dependencies, related
links and epics reuse existing domain apply/conflict and ordering reconciliation.
Project create/rename/delete, label create/rename/delete/restore and workspace
create/rename reuse the existing administration reducers. Workspace operations are
database-wide: the entity is the workspace, and payload workspace fields are refused.
The existing recurrence vocabulary is supported: series creation, template/metadata
updates, projection, outcomes, pause intervals, state changes and stop, including
domain conflict resolution. `domain::payload_keys` covers every `change_log::op_type` name, pinned by a test;
payload semantics stay with wire validation and the reducers. Attachment mutations use authenticated Ref/Unref.
`prepare_encrypted_push` is the single ordered head owner: it returns the frozen record
or preflights pending work and freezes the head, staging exact image ciphertext
with the Ref in the same transaction. Both task and image records pass the canonical
round trip before freezing. Unsupported compound work blocks rather than uploading
only its supported parts.
Preflight is capped at 4096 rows/16 MiB. Those limits and bounded signed membership
are internal refusal boundaries, not product policy.

Each new operation gets a random envelope ID/nonce, with exact ciphertext durable
before dispatch. Unknown outcomes retry those bytes. Same-ID different ciphertext
requires fetching and canonically verifying accepted plaintext before ranking
local work. SQLite triggers protect frozen/verified comparison history from undo
or cleanup. Durable accepted records retain remote provenance without replacing
local origin. Acks never move the cursor. Verified local ranks precede incoming
ordered apply; page effects, mappings, liveness and cursor commit together. Bad
pages roll back, and same-ID divergence preserves pending work instead of merging
or inventing a new ID. Membership sequence, content sequence and local_seq differ.

`encrypted_tail::Authority` carries authenticated membership and complete verified
key coverage. Decryption selects the record/object generation; every accepted
operation additionally checks its sequence against the signed generation interval,
including exact-commitment acknowledgements. Fresh envelopes and objects use the
current generation. Historical admitted images keep their original descriptor/key
for reads, reuse and exact repair. Pending rotation permits history pull and
accepted-outcome resolution but no new envelope, upload or repair mutation.

The root resolves every frozen record by Lookup before any upload or resend, in
current context. `reconcile_encrypted_tail_absence` validates a context-bound
absence against exact durable bytes, unchanged source ownership and all observed
acceptance fences. Only a closed generation permits atomic same-ID re-encryption.
The outbox row continuously protects history; attachment preparation/staging and
its source pin are replaced in that transaction. No scheduling witness or second
queue is needed: before commit, retry requires lookup again; after commit, the
replacement is the sole owned representation. `sync_generation` does not change.
Pull-only rounds never prepare or supersede work. One typed stale retry retains
round progress, the selected download and the finite initial watermark.

`encrypted_tail/recurrence.rs` identifies deterministic materialization operations.
Encrypted page apply reuses the recurrence aggregate operations without generating
wall-clock projections at page boundaries or replaying prefix history over the
materialized snapshot. Reaching the current server watermark does not prove an
author's pending history is fully uploaded. Normal local mutations and reports
own clock-driven projection reconciliation. Lifecycle conflict resolution retains
its operation-owned reconciliation at the authenticated `changed_at`, inside the
page transaction. This can generate history needed by a later page record; only
validated deterministic operations with canonical equality can rank that pending
history as an echo. Retained prefix identities never become tail submissions.
Concurrent completion shares successor identities. Concurrent generation from
different templates can produce unequal meaning under the same deterministic ID;
this remains an explicit integrity failure with pending evidence retained, not an
identity-only acknowledgement or automatic conflict repair. Series lifecycle and
outcome changes do not delete task rows or image references. Occurrence creation
uses the existing Parent creation projection, and explicit task deletion uses the
existing conservative Parent deletion projection.

`encrypted_tail/notes.rs` reconciles each affected note from retained tail commands
in accepted sequence order, followed by pending local push order. Verified outcomes
and page application both reconcile inside their transaction, including local
echoes. Prefix history is not replayed over the published note baseline. Edits
replace bodies only while a note exists; deletion requires a later add (including
undo restoration) to recreate it. Replay preserves the establishing add identity
and timestamp and does not repeat task activity writes. Retaining these tail
commands is required; pruning them needs a separate confirmed-baseline contract.

`encrypted_tail/labels.rs` assigns each affected task-label pair from its last
retained tail command, using accepted sequence order followed by local pending
push order. Label deletion and rename away assign absence, and restoration listing
the task assigns presence. A rename into the label keeps the materialized presence.
A label whose last tail command deletes or renames it re-applies that command
through the existing reducer, then reconciles the renamed label's pairs. Accepted-only outcomes and incoming pages, including local echoes,
reconcile transactionally. Pairs without tail commands keep their published
materialization, not a replay of prefix history. This uses the same retained-tail
requirement as notes and does not change dependency cycle arbitration. A rename that
races a remote per-task change to the old label keeps apply-order presence for the
new label, as plaintext sync does.

`encrypted_tail/dependencies.rs` rebuilds each affected workspace graph from a
local association-lifetime materialized baseline plus retained dependency tail
commands, in accepted sequence order followed by pending push order. It reuses
the existing sequential cycle arbitration without replaying other domain actions
or prefix history. Seed adoption initializes the baseline from the authenticated
capture, never post-capture live state; peer installation uses the verified
snapshot. The baseline identity and edges commit with adoption/installation and
are excluded from portable state. Exact retries validate rather than replace it.
Missing or mismatched baselines require explicit reinitialization; no inferred
repair is supported. Outcome/page graph, history and cursor effects are atomic.
Replay batches command IDs but still revisits retained graph history; retention
and production-scale replay budgets require a separate confirmed-prefix contract.

Authenticated None/Parent projections carry only minimal parent-retention inputs.
`persistence/parent_liveness.rs::ParentState` is shared by plaintext retention,
bootstrap projection and encrypted admission. Captured parent state is the tail
baseline. The keyless server reduces Parent exactly once on new acceptance,
atomically with allocation and affected image grace timestamps. Retries do not
repeat it; force resolution never clears sticky protection. Ref/Unref admission shares this reducer and its grace predicate. Snapshot bytes
are never rewritten.

Run `cargo test --lib 'encrypted_tail_http::tests::'` for the real seed publication,
independent enrollment/install and bidirectional task harness. It includes exact
retry, server reopen, client subprocess exits, conflicts, retained images, invalid
page rollback and explicit refusal tests. Controlled same-ID fixtures exercise
canonical verification. `tests/recurrence.rs` also exercises actual independently
generated deterministic successors, snapshot continuation, lifecycle and outcome
conflicts, malformed compound rollback, lost acknowledgement and image retention.
`tests/administration.rs` covers project, label and workspace administration
across peers and a fresh installation. Rotation, recovery, shipping setup, UI, iOS and comprehensive integration review
remain separate work.

### Internal encrypted attachment transfer

`sync/encrypted_tail/attachments/{codec,client,server}.rs` owns immutable image
recipes, local preparation and verified installation, and keyless server object
admission. It reuses the image chunks and generation/object key derivation in
`shared_state/package.rs` and the publication Artifact codec. Plaintext hashes
remain in local CAS and encrypted domain content, never server descriptors.

Fresh publication initializes descriptor/provenance ownership on existing opaque
image tables; adoption and peer installation initialize association-scoped local
mappings, including explicitly unmapped unavailable references. Exact retries
validate initialization. Missing initialization in older bound development DBs
refuses transfer without backfill, reinstallation or deleting domain data.
The dependency baseline remains a separate local association-lifetime owner.

Preparation pins source plaintext and immutable history and freezes exact image
and operation records before dispatch. Saved-nonce reconstruction hashes an owned
source buffer before encryption and verifies all frozen commitments before use.
Accepted representation comparison remains independent of canonical domain
comparison. Verified outcomes adopt the accepted mapping, never the abandoned
prepared one. Insert-once references retain deletion tombstones; task undo uses
existing local-history fences or task deletion, not reference resurrection.

The server stores opaque chunk bytes in SQLite, not plaintext CAS files. Scoped
current-membership authorization, immutable descriptors, storage epochs and
caller-owned expiring reservation tickets guard PUT, completion and Ref admission.
Ref requires verified complete bytes and live same-workspace reuse or a valid
capacity promise. Accepted operation retries bypass new storage admission even
after Unref or pruning. Hints can only add sticky parent protection. Shared-object
quota counts distinct protected objects plus nonduplicated reservations;
restoration/protection may exceed quota without losing existing promises.
Bounded transactional pruning deletes chunk rows, advances the storage epoch and
retains descriptors/provenance/reference tombstones. Published image catalogs are
not permanent image-byte pins. SQLite/WAL physical size is not logical quota.

`src/encrypted_tail_http/images.rs` supplies isolated `/e2ee/images/v1` transport
and the single ordinary `Client::round`. Each call pushes at most one ordered head,
applies one metadata page and downloads at most one image. The caller supplies the
local blob directory. `metadata_caught_up` reports remote-watermark completion and
local metadata idle; `images` separately reports pending, failed or unavailable
images. An unavailable local image source or failed image upload reports images
Failed and leaves that head pending while the page is still pulled. Other preparation
refusals, integrity violations and same-ID divergence remain round errors. Download
failure never rolls back committed metadata.
Fresh peer installation starts a local initial-image catch-up marker as pending.
The first validated tail page atomically captures its finite watermark with page
effects. Subsequent bounded rounds request that same watermark until reached,
then mark the initial catch-up complete. Restart and exact install retry preserve
this state; later appends do not extend the target. Seed adoption begins complete.
Missing bookkeeping fails closed, without inferred backfill. Core download
selection refuses before initial catch-up, and the host reports images Pending.
Full decoder facts, hash, length, AEAD and frozen commitments precede availability.
A local association-scoped selection cursor advances before each download attempt
and wraps through pending objects, so failed/unavailable images cannot starve later
ones across client or process restarts. Pending-work observations do not advance
this cursor. It is scheduling bookkeeping, not validation or a sync watermark.
`encrypted_round_state` is the single validated observation of cursor, finite
watermark, local idleness and upload/download demand; demand is computed only after
initial catch-up.
Local downloads retain existing local capacity policy; the internal adapter uses
its defaults. `router_with_policy` accepts operator-owned server policy, with the
ordinary server's 30-day/10 GiB defaults, not local seven-day grace. Tickets reuse
the ordinary ten-minute TTL, not bootstrap's 24-hour staging reservation.

Targeted `Client::repair_attachment` reconstructs only a known authenticated
mapping and obtains protection before uploading exact bytes. Automatic repair
scans, background scheduling, disk-backed ciphertext, rotation,
recovery and shipping CLI/mobile integration remain outside this path.

Focused tests: `cargo test --lib 'encrypted_tail_http::tests::attachments::'`,
`cargo test -p aven-core --lib 'sync::encrypted_tail::attachments::'`, the existing
tail/bootstrap/install suites, and plaintext attachment lifecycle regressions.

### Bootstrap publication format and local package ownership

`crates/aven-core/src/sync/shared_state/package/publication.rs`, exposed as
`sync::bootstrap_format`, owns the bounded provisional descriptor, three clear
catalogs, encrypted typed-domain stream and keyless structural validator.
Its module documentation specifies the exact experimental byte profile. Children
own binary framing, catalog checks, domain DTOs and captured image/reference
projections. Domain DTOs are independent of public export compatibility; explicit
conversions reuse shared snapshot validation without a legacy server URL.

The local durable package owner freezes the capture's bootstrap/stream identities
and exact bytes in SQLite. Reopening loads those bytes rather than regenerating
ciphertext or rereading live domain state, and verifies every record against the
committed descriptor and catalogs before use. Client validation compares decrypted
history and image mappings to the capture. Keyless validation checks only public
structure, commitments and selected bytes, not arbitrary domain validity or
membership authority. `publication/staging.rs` exposes crate-private structural
views for server staging using the same descriptor, catalog and chunk validators.

### Authenticated bootstrap staging

`sync::bootstrap_staging` exposes `Database` methods for declaration, bounded
chunk PUT, read-only status, ensure/resume, reclamation and terminal cancellation.
Every operation reads the admitted signed genesis and authenticates its bearer
and exact vault/genesis context within its transaction. Declaration additionally
binds generation and membership predecessor to that genesis. The descriptor
freezes bootstrap/stream identity and budgets; all PUTs and reclamation name its
commitment and current candidate epoch. IDs, descriptor possession and setup
credentials do not authorize staging.

`server_seed_claim.genesis_only` gates unpublished staging and claim retries.
Publication retires it in the same transaction that installs an explicit current
membership head. Published status, exact publication retry and cancel-to-outcome
require that head to match the supported signed membership chain. A historical outcome
alone never authorizes a credential. Unknown successor heads fail closed; no
general membership, credential replacement or rotation engine is implemented.

`server_bootstrap_candidates` owns one active frozen descriptor, expiry and
candidate-scoped epoch, plus terminal bootstrap-ID cancellation tombstones.
Cancellation works before declaration and releases unpublished staged blobs
atomically. Publication wins return the immutable outcome without deleting data.
Only the currently authorized seed can allocate or reclaim unpublished staging. `server_bootstrap_chunks`
stores exact bytes and presence in the same SQLite transaction, without staged
files or a filesystem cleanup protocol. Failed writes roll back both bytes and
presence. Status allocates nothing and distinguishes missing, quarantined and
verified chunks; data/image components become known only after their describing
catalog verifies. Verification establishes public framing/commitments, not AEAD
or domain correctness.

Each catalog is quarantined until its complete ordered bytes pass aggregate and
structural checks. Failure records its class/reason, advances the epoch and drops
quarantine, retaining verified candidate-owned artifacts. Ensure revalidates
retained bytes and budgets. Expiry requires ensure to renew the reservation and
fence old writers; it is not cancellation. Authenticated reclamation checks its
expected epoch and serializes deletion with PUT and resume. Only actual
reclamation removes verified presence.

Declaration bounds are 600 MiB of chunk payload and 4,096 chunks, including the
three catalogs, plus the codec's 1,024-byte descriptor cap. One PUT is at most
1 MiB plus 222 framing bytes. A 24-hour reservation bounds PUT eligibility;
1,024 lifetime candidate/tombstone identities bound retained metadata, refusing
new identities rather than forgetting terminal outcomes. These are logical
storage bounds, not total SQLite/WAL/disk or concurrent-request memory bounds.
Validation materializes at most one bounded encrypted artifact and catalog at a
time. SQLite atomicity/restart tests do not establish power-loss durability.

### First-device authority and signed initial publication

`sync::seed_claim::Genesis` accepts only sequence-zero genesis with one device,
one initial generation, zero predecessor and no bootstrap, recovery or pending
rotation. Its exact immutable format is separate from the fixed publication
successor in `seed_claim/publication.rs`. These are provisional protocol format
boundaries, not a released wire contract or security approval.
Strict Ed25519 authenticates the fixed record and an HPKE base-mode self-package.
The host decrypts and validates self coverage against its existing protected
package secret before persisting seed authority. The keyless server checks public
structure and signature, not generation entropy or encrypted self-package meaning.

`ProtectedLocalKeyStore::prepare_seed_claim` retains the existing vault/generation/
secret and saves signing/recipient private keys, client bearer and exact genesis
in a separate bounded protected record under the same installation lock. A
nonsecret marker detects missing seed authority across same-path DB replacement;
`local_seed_genesis_pin` additionally detects loss when the task DB survives.
Partial marker or DB-pin writes resume existing protected bytes, never regenerate.
`package_seed_capture` binds a package to the pinned genesis. Incompatible frozen
package context requires explicit never-dispatched cancellation/recapture, without
replacing protected keys or rewriting frozen commitments.

`Database::admit_seed_claim` serializes singleton occupancy, authorization and
immutable record persistence with `BEGIN IMMEDIATE`. First admission requires
operator-configured setup authority, never a self signature or proposed bearer.
Identical retries require that setup authority or the stored seed verifier's
bearer; divergent records cannot replace authority. The result is an equality
check against locally pinned intent, not READY or current membership proof.
`server_seed_claim` stores only the public signed record, including encrypted self
coverage and verifier. Publication retires the stored `genesis_only` gate in
its own transaction. Sequence-zero claim retries remain retired after publication.
The seed HTTP router consumes these APIs.

`SeedAuthority::prepare_bootstrap_publication` authenticates the already-frozen
package against its protected genesis and generation key before signing the
fixed sequence-one successor. The signed tuple binds bootstrap, stream, exact
descriptor and manifest commitments, and prefix boundary N. The public validator
resolves the signing key from genesis and derives the entire expected resulting
state, preserving device credentials and generation fields exactly. This is
preparation only: no protected checkpoint advancement, dispatch ownership or
adoption. The local `never_dispatched` journal remains unsuitable for production
dispatch and must not be silently promoted by a host caller.

`Database::publish_bootstrap` owns one immediate transaction for current-head
and bearer authorization, predecessor/epoch/expiry checks, all three complete
catalogs, every selected current and extra image, declaration budgets and
operator-owned workspace quota, signed history and immutable outcome, active
prefix and image projections, allocator=N and READY. Completeness reuses the
publication codec, not a caller list or staging presence flags alone. The keyless
server validates exact ciphertext framing and commitments, never domain plaintext
or AEAD correctness. Empty prefix is valid; the durable high-water mark reserves
1..N even without tail rows. `encrypted_tail/server.rs` allocates only above that
mark and refuses captured prefix IDs rather than returning a duplicate success.

`server_bootstrap_publication` retains exact signed intent and descriptor;
`server_e2ee_membership_head` independently identifies current authority. Exact
retries authenticate first, ignore obsolete staging epochs and mutable allocation,
and return the retained result without recreating image ownership. Read-only
status creates no reservation. Unsupported successor authority is explicitly
rejected rather than falling back to historical seed credentials.

Published descriptor, manifest, state/history and catalog bytes stay lifetime
roots. Publication moves image chunks out of candidate staging into
`server_e2ee_image_chunks`, with immutable catalog-backed object provenance and
ordinary reference/parent ownership in `server_e2ee_images`,
`server_e2ee_image_references` and `server_e2ee_image_parents`. Unprotected extras
start grace at publication; contested or unknown parents retain protection even
when their objects are selected extras. Metadata-only unmapped references create
no byte obligation. Workspace usage is distinct protected ciphertext objects,
not snapshot roots or grace bytes. Bounded opaque-image pruning belongs to `encrypted_tail/attachments`.
Staging ensure, PUT and reclamation cannot mutate published data.

Core tests exercise actual crypto and SQLite, including independent pools,
write-failure rollback and process exit after commit before response. Isolated
host tests use restricted file-backed protected authority and frozen images.
Separate host HTTP tests cover real loopback and client/server process restart.
Neither test family proves TLS deployment, filesystem power-loss, Apple Keychain
lifecycle or physical-iPhone behavior, nor a complete two-client E2EE loop.

### Seed bootstrap HTTP construction

`src/seed_bootstrap_http.rs` owns a dedicated Axum router and Reqwest client. Its
isolated server database and router have no plaintext endpoints or shared-token
fallback. Operator setup authority is accepted only by initial claim; staging,
status, ensure/resume, terminal cancellation and publication always call core
current-membership authorization with the device bearer. The client never
cancels uncertain work automatically. Only `aven sync setup` selects this
adapter; configuration and the daemon never do. Without an explicit setup authority, claims use the
storage's unexpired verifier issued by `aven server setup` and kept in `meta`,
read inside the claim's writer transaction so a reissue cannot interleave.

The provisional framing is POST `/e2ee/bootstrap/v1` with a bounded JSON context
and operation envelope. Existing descriptor, catalog, chunk and signed-record
bytes are byte arrays, not re-encoded crypto records. Credentials are sensitive
Authorization headers; IDs and payloads stay out of URLs. The module defines
encoded body caps, a 30-second request timeout and one concurrent request per
router, with bounded static refusal bodies. Clients allow HTTPS or loopback HTTP,
disable redirects/proxies/content decompression, and incrementally cap responses.

After explicit protected claim authority, source binding, capture and packaging,
`Client::claim` verifies the real server claim result against pinned genesis.
`Client::resume` seals the durable protected intent, authenticates the exact frozen
upload via `Database::seed_publication_upload`, reads remote status, and explicitly
declares or ensures staging before bounded exact PUTs. It retries stored bytes,
not live task/image inputs, with current staging epoch. Remote publication bytes
pass the core signed-record validator with the locally retained descriptor before
adoption. Post-adoption capture cleanup stays in the existing protected host API.

Run the isolated real-server harness with
`cargo test --lib 'seed_bootstrap_http::tests::'`. Its subprocess workers exercise
server exit after committed PUT and publication before response delivery, then
new client/server processes resuming the same candidate and protected intent.
Negative HTTP tests exercise authority/context, bounds, immutable-byte and epoch
refusal, and invalid outcome rejection before local adoption. File-backed test
authority does not access production Keychain items. Encrypted ordinary tail,
fresh join, pairing/recovery UX, general membership and iOS remain outside this
transport boundary.

### Sync CLI, daemon and TUI entry points

End-to-end encrypted sync is the only sync mode. `src/sync/encrypted.rs` wires
the existing host APIs to `aven sync setup`, `sync invite`, `sync join`,
`aven sync` and `sync status`. A seed genesis pin or enrollment pin makes a
database set up (`is_set_up`); any other database stays local, `aven sync` and
`invite` fail with `sync-not-set-up`, and status reports `not-set-up` without
reading protected keys. The server is the locator bound into protected
enrollment identity, never configuration. Setup reruns resume: once a sealed
publication intent exists, setup skips claim and capture. Invite polls
`admit` and join polls `complete`, each acquiring the sync coordination lock;
status takes that lock too because protected inputs hold the installation
exclusively. `drain` repeats `Client::round` up to a caller budget, stopping
when metadata is current and images settle or after 16 consecutive failed or
unavailable image rounds. `run_to_completion` (CLI and TUI) waits for the
coordination lock and uses `ROUND_LIMIT`; `daemon_round` tries the lock once and
uses the daemon budget. `create_invitation` and `await_admission` are shared by
`aven sync invite` and the TUI `:add-device` controller. Status is local and reports metadata separately from
image uploads, downloads and unavailability. An unused outbound invitation
pauses ordinary rounds, and rerunning invite resumes it. `active_inputs`, under
the store lock that also covers admission preparation and dispatch, retires an
expired invitation with no `sent-*` marker through a protected `retired` phase;
admission refuses retired handles and invite then creates a new invitation.
Once a grant may have been sent, expiry never retires it. After expiry,
`finish_pending_management` reconciles the journal from verified membership:
a candidate at its signed slot finishes `ready`. Otherwise it writes
`withdrawing` (no further candidate or resend), sends the authenticated server
`Cancel`, which fences new admission of that registered handle, and runs a
withdraw-bound management intent: a targetless Revoke freeze, then the existing
Rotate. `withdrawn` requires every stored candidate to have lost its slot, no
candidate recipient ever becoming a member, and a later non-pending generation
absent from every candidate predecessor; an existing qualifying rotation is
reused. Historical keys the grant carried stay disclosed. `invitation.rs` encodes device
invitations as the pairing spec's `aven://pair/v2/` URI and setup invitations
in the provisional `aven-sync-setup-1:` form, both carrying the validated
server origin.

`aven server setup` stores one expiring setup verifier in server `meta`,
refusing claimed storage or storage with change history. Reissue keeps the
stored setup ID in the same transaction and replaces only the verifier and
expiry, so a device genesis bound to that ID can still claim. `aven server`
serves only the seed, enrollment and tail/image routers, binds loopback only,
and requires prepared storage; storage with change history is refused as
unsupported. Server-side opaque-image pruning is not scheduled.

`ProtectedLocalKeyStore::for_database` is the production credential boundary.
In `cfg(test)` builds it uses the isolated file backend named by
`AVEN_TEST_PROTECTED_KEYS` and otherwise fails, so tests never reach the login
Keychain. `sync::encrypted::tests` runs real commands through `run_cli_from` in
re-executed test-binary workers against an `aven server` worker, including
conflict commands and a woken `aven daemon`. Integration tests under `tests/`
run the release binary and so cover only keyless paths (server setup and
launch, not-set-up status). Focused tests: `cargo test --lib 'sync::encrypted::'`.

### Pure membership and generation validation

`seed_claim/membership` owns one predecessor-derived validator for AddDevice,
Revoke and Rotate. `encoding.rs` owns bounded AVGS5/AVGA5 framing;
`rotation.rs` owns active-target removal, pending preservation, exact sorted
recipient coverage and signed generation cutoffs; `keys.rs` yields complete
commitment-checked `VerifiedKeys` only after recipient validation. Pairing grants
cover every predecessor generation, while rotation packages cover exactly the
new generation and extend existing verified coverage. Retired device/signing/HPKE
identities remain unavailable for reuse. Only Rotate clears pending; final-device
removal is refused. Capacity checks preserve a pending rotation's generation,
transition and worst-case record budget.

`evidence.rs` bounds and replays mixed public transition evidence, retaining
original enrollment validation at its historical predecessor. Genesis/publication
bytes and bootstrap identity remain unchanged. These pure APIs establish signed
intent and key coverage, not server commit, protected storage or runtime readiness.
The server journal admits AddDevice, Revoke and Rotate. Protected refresh retains
complete historical key coverage across these transitions. Ordinary host rounds
use that coverage and reconcile frozen work across cutoffs. Ordinary host rounds
finish pending rotations through the bounded management dispatcher; pull-only
rounds never dispatch management. Shipping removal/setup UI remains a separate
integration gate.

### Transactional repeatable membership

`seed_claim/membership/persistence.rs` owns bounded complete signed history,
handle-keyed invitation/request/outcome storage and the vault clock high-water.
Each operation replays signed predecessors before authenticating the current
credential. Only the evidence read permits a verified ancestor request context;
admission and exact historical outcome retries require the current head.
A single transition journal and head commit with invitation outcomes. Revoke
atomically denies removed credentials, terminally expires their unused invitations
and deletes their image tickets; Rotate requires the exact unchanged allocator H.
Management preparation returns authenticated bounded evidence and H. Historical
outcomes require current authentication, while surviving public enrollment
mailboxes retain only their immutable original grant after inviter removal.
Unsupported admission-only stores refuse without conversion or domain deletion.
The tail resolves accepted IDs before freeze/generation rules; new operations
require unfrozen current generation. All image mutations, including release and
prune, are frozen while rotation is pending. Historical object access/repair/reuse
requires an exact descriptor with bootstrap or accepted-tail provenance; complete
never-admitted old-generation uploads do not qualify. SQLite invitation indexes are projections, not
positive authority. Protected ownership and transport select the runtime owner;
the persistence API itself does not establish host readiness.

Focused tests: `cargo test -p aven-core --lib 'sync::seed_claim::membership::persistence::tests::'`.
They include independent-pool admission/management races, SQL write faults, same-recipient
successor preparation, repeated peer invitation, retained expiry and clock
rollback, and invitation/evidence bounds. These are not HTTP or protected-store
power-loss evidence. Content and image cutover/race tests live under
`encrypted_tail/server/rotation_tests.rs`; run `cargo test -p aven-core --lib
'sync::encrypted_tail::'`. The isolated enrollment HTTP adapter exposes management
preparation/application; `peer_enrollment_http/management.rs` owns the bounded
client removal/finish dispatcher.

### Protected enrollment, checkpoints and refresh

`seed_claim/peer.rs` contains shared PSK request, PoP, HPKE and independent-key
primitives. `seed_claim/membership` is the sole live admission validator, using
predecessor-derived membership from sequence two onward. Any installed peer may
invite with its own signing, recipient and bearer keys. Old enrollment formats
and internal protected stores refuse without conversion or domain deletion.
Genesis/publication and ordinary content/image encryption remain unchanged.

`src/protected_local_keys/peer.rs` separates immutable inbound identity and original
verified enrollment/install receipts from handle-keyed outbound journals. Every
journal protects exact request binding, candidate and Sent facts before disclosure.
Only a verified different successor at the candidate's signed slot permits a new
candidate for the same recipient. Unfinished invitations block ordinary dispatch,
including on already-installed peers. Expiry does not withdraw a disclosed key or
clear this fence. Safe management may proceed through this fence but cannot
clear it or grant ordinary dispatch readiness. Signed cancellation and recovery
remain outside this owner.

`src/protected_local_keys/membership.rs` owns append-only protected checkpoint
records and commitment-addressed public evidence outside replaceable SQLite.
Each checkpoint binds verified ancestry and the digest of complete protected
historical generation coverage. The same installation-bound append-only owner
persists coverage before its checkpoint and SQLite loss-detection mirror. Core
`VerifiedKeys` revalidates the exact ordered generation set and every commitment
on load. Refresh replays each intervening transition and opens the device's own
rotation package; fresh enrollment supplies every historical key. Pending state
retains old coverage. A verified signed removal can persist a denying checkpoint,
but an arbitrary server error cannot establish removal. Protected-ahead recovery
moves only forward; missing established coverage, ancestry or a conflicting
mirror refuses without reconstructing trust from SQLite or the network.
Installation and store exclusion cover management exchanges and ordinary rounds.
Original installation receipts, association, cursors, dependency baseline, image
mappings and initial finite catch-up watermark never become mutable membership.

`src/peer_enrollment_http.rs` provides bounded Membership evidence reads using the
active credential and a known verified ancestor context. All content requests
still require the current head. Hosts refresh before management/ordinary rounds
and retry at most once for a typed stale context, never for arbitrary integrity,
authentication or network errors. Retry retains completed metadata work and the
same selected image rather than advancing the download selector again. Fresh join completion pins the PSK-authenticated
outcome before retrieving full ancestry and can finish after later admissions.
Every published component read is independently authorized; refresh preserves the
immutable component and original install checkpoint. Fresh installs always decrypt
the immutable bootstrap with its original generation key, including when enrollment
or subsequent refresh covers several newer generations. Completed install retries
remain local and never reset later domain edits.

Focused tests: core membership tests, `peer_enrollment_http::tests::`,
`encrypted_tail_http::tests::membership::` and existing tail/image suites. The
three-client fixtures use independent databases, protected stores and blob roots,
real loopback HTTP and core mutations. `peer_enrollment_http::tests::rotation::`
covers offline multi-rotation coverage, fresh historical bootstrap installation,
immutable receipt retries, protected-before-mirror failures and missing/corrupt
coverage. `encrypted_tail_http::tests::membership::rotation::` covers server-driven
removal with surviving root task/image rounds, cutover outcomes, signed interval
negatives, frozen reads and atomic supersession across subprocess exits. SQL faults and subprocess exits are not
power-loss, Keychain, mobile, interoperability or independent security evidence.

### Durable removal and automatic rotation

`src/protected_local_keys/rotation.rs` reuses the installation-bound append-only
phase owner for bounded removal/finish intents, predecessor/cutoff plans, protected
rotation material, exact signed candidates and Sent/Ready commitments. SQLite
stores only artifact digests. Complete signed ancestry resolves a candidate's slot
before any replacement: no successor retries exact bytes, a matching successor is
committed, and another signed successor proves loss. A losing Rotate replacement
owns fresh generation/key/HPKE randomness. `RotationMaterial` is protected-only
storage framing, not a network codec or an alternative key-coverage authority.
Original enrollment, bootstrap and installation receipts stay immutable.

`src/peer_enrollment_http/management.rs` holds the existing installation/store
locks across refresh, authenticated preparation and bounded exact dispatch.
`remove_device` persists one target intent and attempts Revoke then Rotate.
Ordinary metadata/image rounds resume local work and can finish another device's
freeze, with at most one typed stale retry. A second race fails without discarding
the retained candidate. Pull-only and explicit image repair remain read-only with
respect to management. Last-device refusal occurs before creating an intent;
self-removal can confirm an in-flight acknowledgement but cannot use a retired
credential to resolve a lost reply. Local data and keys remain retained.

Focused tests: `encrypted_tail_http::tests::membership::management::` uses actual
root client entries and independent installed peers for removal, task/image
continuation, lost/undelivered replies, reopen, competing finishers, fresh losing
secrets, second-race bounds, disclosure fences and protected-phase process exits.
The process-exit worker is exercised by its parent test. These are loopback and
file-backed protected-store results, not platform power-loss or security approval.

### Sync flow

1. Local mutations append operation-log rows in `changes`. Both ordinary and explicit-identity insertion helpers enforce the persisted replica protocol through `sync/protocol.rs` inside the owning transaction, before sequence allocation. They also share the wire validator's 64 KiB serialized UTF-8 JSON payload limit and return typed validation errors inside the owning mutation transaction. Existing operation history is preserved.
2. Unsynced rows have `server_seq IS NULL`; an accepted encrypted tail record assigns `server_seq`, and `sync_cursor` advances only after a validated page applies.
3. `src/sync/coordination.rs` derives a persistent sidecar from the canonical SQLite filename. Its kernel-owned exclusive lock covers the complete root host operation and releases on guard drop or process exit. Interactive and TUI sync wait for a short bounded interval. Daemon sync attempts once and defers on contention. In-memory databases bypass filesystem coordination.
4. Each round in `src/encrypted_tail_http.rs` refreshes membership, freezes and seals pending changes, uploads and downloads encrypted image objects, pulls encrypted tail records and applies them through `crates/aven-core/src/sync/encrypted_tail/` and the shared `sync/apply/` operations, including conflict generation and epic, label, note, dependency and recurrence reconciliation.
5. `encrypted_tail/domain.rs::payload_keys` covers every `change_log::op_type` name, pinned by a test; an operation outside that vocabulary stops ordinary rounds with `encrypted-tail-operation-unsupported`.
6. `src/daemon.rs` runs one budgeted `daemon_round` per wake. Contended rounds reschedule without failure output; an unset database waits at the sync interval.

## Data ownership

SQLite stores synced task data and local UI state. Config files store local routing and service settings.

- Synced domain tables: `workspaces`, `tasks`, `projects`, `labels`, `task_labels`, `metadata_fields`, `metadata_field_id_aliases`, `task_metadata`, `notes`, `task_dependencies`, `task_epic_links`, `task_related_links`, `task_attachments`, `recurrence_series`, `recurrence_series_labels`, `recurrence_series_metadata`, `recurrence_occurrences`, and `recurrence_pause_intervals`. Recurrence occurrence rows are sparse links and explicit outcomes. Archived projections retain their task rows and use `projection_state = 'archived'`.
- Attachment object state: `blob_inventory`, `blob_lifecycle`, `blob_leases`, `blob_upload_reservations`, `server_blob_references`, and `server_task_tombstones`; encoded image bytes live in content-addressed files under the configured blob directory. The `local.image_optimization` values are `off`, `paste`, and `on`. `off` is the default, `paste` optimizes paste sources, and `on` optimizes paste sources and file attachments. Lazy terminal thumbnails live under disposable `cache/previews/` storage.
- Sync and local client bookkeeping: `changes`, `field_versions`, `conflicts`, `shared_history_provenance`, `local_shared_capture_journal`, `local_shared_capture_changes`, `local_shared_capture_images`, `local_shared_capture_pins`, `local_shared_capture_packages`, `local_shared_capture_package_chunks`, `local_shared_capture_package_images`, `local_shared_capture_package_image_chunks`, and `meta`. `shared_history_provenance` retains each installed prefix change's original accepted sequence or pending rank separately from its dense local prefix rank. The local capture tables own one immutable, versioned, never-dispatched snapshot, stable candidate and stream identities, the exact protected history map, current/extra/unavailable image classification, non-expiring managed-cleanup pins, and optional frozen encrypted package records. Package records use the provisional XChaCha20-Poly1305 and HKDF-SHA-256 chunk profiles with caller-owned key material and a 1 MiB chunk size. Local package limits are 256 MiB for state, 1 MiB for the encrypted manifest, 25 MiB per image, 1,024 selected images, and 256 MiB of selected image plaintext in aggregate. These are local resource limits, not frozen network limits. Packages contain shared database state, retained history, and every capture-selected available image. Validated unavailable metadata is retained without bytes. While present they fence ordinary client sync, hard history undo, import, supported backup, and restore. Only the explicitly local never-dispatched cancellation API removes them, and it never removes domain rows. `meta` stores sync cursors, the local sync-generation fence, `sync_established_protocol`, `sync_blocked_protocol`, client identity, the stable iOS queue workspace ID, and durable local TUI scalars such as the onboarding version and independent sidebar section collapse preferences. Sidebar preferences are database-wide across workspaces and projects, default to expanded, and load through `TuiStore` before initial sidebar projection. `ListSurface` owns runtime collapse state; application actions save each changed section through the store and core local-state boundary with nonfatal warnings on load or write failure.
- Protected local package authority: `src/protected_local_keys.rs` stores one vault/generation/package-key record outside SQLite and ordinary Aven backup inputs. macOS uses a non-synchronizing login Keychain item plus a nonsecret state marker. Linux uses an owner-only keyring file plus marker under `$XDG_STATE_HOME/aven/protected-keys` or `~/.local/state/aven/protected-keys`. Records are keyed by a hash of the canonical database path, so supported replacement at the same path preserves authority while a copied database at a different path is a distinct installation. Missing established material, unavailable storage, unsafe permissions, and corrupt records fail closed without replacement.
- Local-only config: database path, sync settings, project path mappings, directory overrides, and TUI column grouping.
- Local-only TUI state: view, filter, selection, overlay, sort state, and `tui_undo_entries`; pending undo entries are cleared when a TUI store starts.
- Backup and portability workflows: `src/commands/data_safety/mod.rs` owns filesystem selection and presentation, while `crates/aven-core/src/data_safety.rs` and `crates/aven-core/src/data_safety/` own scans, validation, import, integrity, and backup mechanics. Portable exports include recurrence series with generic frequency and interval values, template labels, sparse occurrences, pause intervals, generalized conflicts, generalized field versions, and shared-prefix provenance when present. They exclude local capture authority. Imports retain the source server pin when accepted history belongs to legacy sync. Accepted history without a valid source server identity is rejected unless the current export version carries complete validated shared-prefix provenance, which is preserved on import rather than silently associated with a server. Older task-only exports create no recurrence linkage. SQLite backups run in-process with `VACUUM INTO` on an owned connection and install the completed staging database at the destination. Archive backup holds the writer gate across the database snapshot, attachment lease lifetime, and object staging. The completed staging database owns archive inventory, manifest entries, and object selection. Supported backup and restore refuse a database containing active never-dispatched capture authority, so a copy cannot become a second local publisher and replacement cannot discard protection. Backup checks the completed SQLite staging snapshot before exposing it, rather than trusting only a live-source precheck. SQLite restore otherwise validates its source, creates a consistent safety backup, replaces the database file, and removes stale WAL and shared-memory sidecars. Raw filesystem copying and replacement while processes run remain unsupported.

`Task` and `Project` in `crates/aven-core/src/types.rs` are core records. Workspace-scoped tables include `workspace_id` in uniqueness and lookup paths. Many invariants are application-enforced rather than database-enforced, so do not write domain tables directly unless a change is intentionally bypassing sync and validation.

## Domain rules

- Status values are `inbox`, `backlog`, `todo`, `active`, `done`, and `canceled`.
- Priority values are `none`, `low`, `medium`, `high`, and `urgent`. Assigning `medium`, `high`, or `urgent` promotes an inbox task to todo, including during creation. Other statuses and priorities do not trigger status changes.
- Task source values are `cli`, `tui`, `api`, `ios`, `android`, and `unknown`. `TaskSource` is a closed enum; unfamiliar values are rejected, not normalized or mapped to `unknown`. Consumer queue and epic creation records the explicit source supplied by its adapter. Existing sources are never reclassified. Only absent values at compatibility boundaries default to `unknown`. Sync protocol 18 requires clients and server to understand `android`; older clients must upgrade before syncing, opening an upgraded database, or importing exports with unsupported sources. Source is immutable and excluded from scalar field versioning, conflict resolution, and undo field mutations. Keep the enum and SQLite source constraint aligned.
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
- Project deletion atomically stops active and paused recurrence series assigned to the project, closes their open pause intervals, retains their final projected tasks, and soft-deletes projects referenced by tasks or recurrence series.
- Labels normalize before storage and must exist before assignment.
- Metadata keys normalize to lowercase ASCII identifiers, are unique per workspace, and reject the reserved `aven.` prefix. Stable `MetadataFieldId` values, not keys, identify task and recurrence-template relations, sync versions, conflicts, and undo. Values are opaque UTF-8 strings, including the empty string, bounded to 4096 bytes each. Task metadata count (128) and total value bytes (32768) are local authoring limits, not replicated-state invariants: valid concurrent remote values may exceed either aggregate. Local task updates and conflict resolutions must leave each metric within its normal limit or not increase it. Shared-state capture/install, portable validation and integrity checks retain valid over-aggregate task metadata. Local input batches and create-task wire arrays retain their own aggregate bounds. Recurrence-template aggregate policy is separate and remains strict.
- Task refs resolve by ID suffix, optionally qualified as `PREFIX-SUFFIX`.
- Typed task ref suffixes must be at least 3 characters. Display refs use at least 4 suffix characters and lengthen to disambiguate current tasks.
- `O` normalizes to `0`, and `I` or `L` normalize to `1` when resolving refs.

## Architectural guardrails

- Use owned `aven_core::db::Database` methods for persisted domain reads and writes from the application package. Operations with orthogonal controls use one parameter type with defaults and builders rather than method-name suffix combinations.
- Keep SQLx pools, connections, transactions, rows, migrations, and query implementation inside `crates/aven-core`. Recovery inspection uses an isolated database snapshot through `Database::inspect`; it does not reuse `Database::open`, enable WAL on user state, initialize metadata or workspaces, run migrations, or create a missing path.
- Keep attachment persistence, validation, content-addressed storage, lifecycle accounting, sync planning and apply, and atomic task-plus-attachment mutations inside `crates/aven-core`. Root application code owns input file reads, output file writes, terminal previews, configuration, Reqwest, and Axum.
- Server attachment retention replays accepted task creation/deletion history in `crates/aven-core/src/sync/persistence/parent_liveness.rs`. Unknown or contested parent deletion protects attachments until their references are explicitly deleted; force resolution does not prove other clients have drained pending work. Server startup rebuilds this projection before maintenance. The tombstone `deleted` column expresses retention eligibility, not necessarily the latest requested task value.
- Task deletion paths accumulate affected attachment hashes during field mutation and reconcile attachment liveness once at the owning write transaction boundary. Sync apply, conflict resolution, and undo follow the same transaction-level rule.
- Use `crates/aven-core/src/operations/` or `crates/aven-core/src/mutation.rs` for writes that affect synced domain data.
- Use `crates/aven-core/src/query.rs`, `crates/aven-core/src/query/`, and core enrichment helpers for read models. Standalone recurrence-aware report entry points reconcile before querying, cap candidates at 256, and may commit one atomic projection transaction per changed series after a slot boundary. Current-projection reads are reserved for a coherent report lifecycle that has already reconciled its workspace. Render-only adapters consume the returned report state.
- Keep scalar task fields aligned across validation, task rows, `changes`, `field_versions`, sync apply, and conflict resolution. Field-version and conflict identity is workspace, entity type, entity ID, and field. Task helpers preserve the existing task-specific API while recurrence series use `entity_type = 'recurrence_series'`.
- Keep workspace scope explicit on queries and mutations that operate on user data.
- Keep config serialization and durable text writes in `src/config.rs`; keep path and server resolution in `src/config/paths.rs`, TUI settings in `src/config/tui.rs`, and custom command settings in `src/config/custom_commands.rs`; keep managed-entry text transforms in `src/config_edit.rs`.
- Keep CLI output formatting in command or render modules, not in persistence helpers. Use `src/render.rs` for shared quoting, changed flag text, multiline blocks, near-match errors, and text diffs. Use focused command-family modules such as `src/commands/context.rs` for command-local snapshots and formatting. Commands with text and JSON output should construct one typed report before selecting a renderer, as `src/commands/prime.rs` does. Task context and full-detail reports load live and deleted attachment metadata once, preserve tombstones in JSON, and filter deleted attachments from human-readable sections.
- Classify every `Commands` variant in the exhaustive `CliDispatch::from` match in `src/lib.rs`. Standalone commands own specialized setup, database commands use the shared SQLite path, and TUI commands use the TUI path. Database dispatch arms resolve workspace context only when their command handler consumes it; command metadata controls logging and daemon wake behavior. Doctor stays standalone so strict config loading, database path resolution, normal SQLite open, and migrations cannot prevent its report. Its inspection path must remain free of user-state writes, daemon wakeups, sync, update checks, and network calls.
- Keep TUI database access in `src/tui/store/` and rendering in `src/tui/ui/`. `src/tui/store/launch.rs` is the database-backed boundary from CLI launch arguments to resolved launch state.
- Resolve raw key and mouse input in `src/tui/input/` against shortcut, capture, and rendered view-model state. Keep routers free of persistence and platform effects. Route overlay mouse events through the typed boundary in `src/tui/overlay/mouse.rs`, with application effects applied by `src/tui/app_overlay_input.rs`. Keep top-level dispatch entry points in `src/tui/app_dispatch.rs`, mouse coordination in `src/tui/app_mouse.rs`, and detail key and pointer coordination in `src/tui/app_detail_input.rs`. Execute semantic actions through the exhaustive table in `src/tui/input/action.rs`.
- `ListSurface` owns browse navigation and selection state. Resolve mutation targets through `TaskSelection`, retain captured target identity for edit flows, and derive read-modify-write values inside core transactions.
- `DetailSession` owns detail activity, interaction, and linked navigation. `src/tui/app_detail.rs` coordinates detail queries and effects. `DetailDocument` supplies shared semantic geometry to rendering and interaction.
- Modal overlays preserve active detail sessions by construction. Authoring and overlay flow states carry every payload required for submit, retry, cancellation, discard confirmation, and underlay restoration.
- Keep TUI attachment sections metadata-backed for committed attachments and app-state-backed for local preparation. `crates/aven-core/src/task_enrichment.rs` loads ordered live attachment metadata, and `src/tui/attachment_controller.rs` owns pending and failure state without publishing a hash. Attachment focus depends on live image metadata and local byte availability, independently of terminal graphics support. `src/tui/preview_controller.rs` owns bounded thumbnail work, caching, leases, retries, and stale-result rejection. `InlineImageSurface` owns terminal placement and backend reconciliation, deferred emission state, terminal cleanup operations, viewer launch policy, and bounded temporary export retention. `src/attachments/export.rs` creates secure leased temporary copies, `src/attachments/save.rs` creates collision-safe durable copies under read leases, and `src/tui/platform/viewer.rs` owns direct platform launcher commands.
- Keep release discovery, cache policy, install ownership, archive verification, and executable replacement in `src/update/`. Keep CLI update presentation in `src/commands/self_update.rs` and TUI update flow state, cancellation, and overlay coordination in `src/tui/app_update.rs`. Never replace package-manager-owned executables or request elevated privileges.
- Debug builds require an explicit database path from `--db`, `AVEN_DEV_DB`, `AVEN_DB`, or `local.db_path`; they never fall back to the release database under `~/.local/state/aven`. `--db` has highest precedence, followed by `AVEN_DEV_DB`, so a worktree's isolated database wins over an inherited `AVEN_DB`. Worktree setup writes `AVEN_DEV_DB`, and direct debug invocations honor it without recipe-specific environment translation.
- Task-list table visibility and order are independent of board lanes. `TableColumn` in `src/config/tui.rs` defines the semantic identities in `ALL` and the default display order in `DEFAULT`; `tui.table.columns` accepts a nonempty ordered subset of `ALL`, and omitting the setting retains `DEFAULT`. Columns outside `DEFAULT`, such as `due`, stay opt-in, so widening `ALL` must not widen `DEFAULT`. Selection and mark indicators prefer `ref`, then `title`, requiring at least three characters of content width in either cell; otherwise, they use a separate row-state gutter. `src/tui/ui/task_list/layout.rs` resolves ordered constraints into frame-local semantic content rectangles, including gutters. Headers, ordinary and epic row cells, inline title editor widths, and status hit testing must consume that shared geometry, never display-position indexes. Keep content-sensitive widths and narrow visibility attached to semantic columns, and keep context-sensitive labels and time values independent of their position. When the dedicated `due` column is visible, the contextual `time` column retains its view-specific metric instead of duplicating deadlines from due-date ordering or urgency.
- Treat TUI board lane names as presentation. Lane configuration (`tui.columns`) must partition the six fixed semantic statuses exactly once so tasks remain visible without changing CLI, queue, readiness, dependency, or sync semantics.
- Derive TUI task filters, query mode, and query-native list render mode from `TaskViewState`. Select list or columns rendering from its independent layout field. Do not keep parallel project, status, query, layout, or queue-sort state.
- Keep live search preview state transitions and worker ownership in `SearchController` in `src/tui/app_search.rs`; keep the active `SearchIntent` and its complete mutation target payload in `SearchState`, search read-model behavior in `crates/aven-core/src/query/`, and overlay rendering in `src/tui/ui/overlays/search.rs`. Search preview reads the current recurrence projection established by the TUI refresh lifecycle and starts immediately for each query.
- Keep natural-add mode, configuration, worker handles, pending and ready states, cancellation, and polling transitions in `IntakeController` in `src/tui/app_intake.rs`; keep authoring coordination in `src/tui/app_authoring.rs` and process construction in `src/tui/natural_add_runtime.rs`. `src/task_intake.rs` produces a typed intake result: ordinary tasks retain `TaskDraft`, while recurrence uses the shared natural rule parser, schedule constructor, authoring fields, and series persistence path. `AuthoringState` owns the active add-task draft and its standalone or epic-child origin for the full flow lifetime.

- Treat project selection in the TUI as scope. Project scope must not be modeled as a filter modifier or view.
- TUI editor, picker, combobox, confirmation, and search states own exhaustive typed intents carrying every payload required to submit or retry the flow. `src/tui/app_overlay_submit.rs` consumes those intents through exhaustive matches. Overlay render projections borrow large resident payloads for one frame and never borrow `WidgetState` or `ListSurface`. They derive presentation kinds for renderers, and titles remain render-only chrome.
- TUI shortcuts use intent prefixes in the command catalog. Navigation and scope use `g`, named views use `v`, composable filters use `f`, ordering uses `o`, editable task fields use `e`, other selected-task actions use `t`, project administration uses `p`, label administration uses `L`, conflicts use `c`, and config uses `C`. The `z` prefix is reserved for configured custom commands and must remain free of built-in bindings.
- The TUI command registry owns built-in and configured command identity, names, aliases, descriptions, semantic classes, and discoverable bindings. Query, resolution, binding, and execution consume that registry through separate typed boundaries. Opening `:` captures an immutable, sum-typed command session with stable workspace, task, mark, recurrence, sidebar, detail-focus, refresh-anchor, and empty-view identities. One registry-backed query supplies rendering, keyboard and mouse selection, completion, and activation. Text relevance precedes availability and contextual ranking. Empty queries omit disabled commands, while typed queries retain them with semantic reasons. Activation rehydrates captured IDs and rejects stale targets without consulting live cursor or focus state. Finite routing domains cover task lists, sidebar rows, detail parent and child surfaces, recurrence lists, and add-task-only mode, so shortcut conflicts and prefix behavior are checked per atomic domain. Custom command target policies resolve from the captured ordered identity projection before JSON planning, preserving aliases, `invoked_as`, target ordering, and validation. Commands that retarget the list or selected task close or rebind `DetailSession` before returning to detail input.
- Picker, command-panel, and text-panel geometry lives in `src/tui/overlay/layout.rs`. Rendering and mouse hit testing derive the same frame-local layout from terminal size and overlay view state; application state never retains terminal rectangles.
- Overlay dialogs should use shared helpers in `src/tui/ui/dialog.rs` for title edges, frame clearing, background, border, and footer hint styling.
- Overlay behavior tests live in the overlay module they exercise under `src/tui/overlay/`; the facade in `src/tui/overlay.rs` stays focused on module wiring and exports. Overlay mouse dispatch owns feature hit testing and local state transitions, and returns typed outcomes for application actions and effects.
- Metadata action and input transitions are tested under `tui::overlay::metadata::tests::`; renderer fixtures and shared chrome assertions live under `tui::ui::overlays::tests::metadata::`. Application metadata tests retain dispatch, captured-target, persistence, and editor recovery coverage. Recurrence metadata creation and committed-refresh recovery belong to `tui::app::tests::authoring::`.
- TUI undo entries are written only by the core operation that owns the corresponding mutation transaction. Core operations expose an explicit no-undo choice for CLI and reusable consumers. They load undo before-values under the immediate transaction, derive summaries from actual changed outcomes, omit undo entries for no-op reports, and roll the full mutation back when undo persistence fails. TUI refreshes compute a coherent replacement projection, including candidate view, workspace state, and command-derived undo presentation, and publish it only after all reads succeed. The application adds an undo action to mutation success feedback only when the refresh identifies a newly available entry. Post-commit TUI refresh failures use the committed-refresh contract, and mutation callers do not reopen retryable edit state. Pending TUI undo entries are valid only within the current `TuiStore` lifecycle and are cleared on store startup.
- Do not log auth tokens, raw sync payloads, task descriptions, note bodies, user-authored labels or project names, protected local key material, or secret config values.
- Keep protected sync authority outside replaceable task databases, settings, exports, and ordinary backup archives. Host adapters must persist required authority before creating dependent ciphertext, preserve an established identity across same-path database replacement, and fail closed rather than regenerate when protected storage is missing, unavailable, unsafe, or corrupt.
- Keep related links represented by one canonical endpoint pair in `task_related_links`. The versioned presence register references the establishing change, resolves synchronized mutations by server sequence, and retains linked and unlinked state across soft deletion. Related links are symmetric context only. They do not affect dependency ordering, epic containment, readiness, scheduling, queue rank, or recurrence templates.
- Keep epic membership represented by `tasks.is_epic` and `task_epic_links`. `crates/aven-core/src/epic_membership.rs` reconstructs membership from retained commands in server-sequence order, followed by the pending local push order. Conflicting additions select the minimum parent ID; removal targets only the named parent and never revives discarded parents. Sync acknowledgements, local echoes, and incoming membership commands reconcile the affected child inside the page transaction. Historical replay preserves the final selected parent's epic flag without repeating task validation or promoting discarded historical parents. Recovery repairs complete retained histories on database open and after import. Snapshot-only memberships retain a per-child baseline in exported local metadata; ambiguous histories beginning with removal without a baseline preserve their projection. This recovery cannot infer missing history. Replay depends on retained membership commands, so change-log retention requires a confirmed-prefix baseline contract. Sync protocol 16 requires participating clients and server to support this reconciliation. Dependency read models represent ordering only. JSON task surfaces include `is_epic`, `epic_parent`, and `epic_children` so agents can keep membership separate from blockers. Task-list enrichment attaches a typed `EpicRollup` to epic items with distinct open, done, and canceled child counts, blocked, overdue, and ready open-child counts, and latest parent-or-child activity. Detailed task hydration attaches each epic child's dependency links separately from epic membership. TUI epic rows, previews, and details consume these shared read models. Epic link mutations accept explicit epic and child IDs. Adding the first child to an ordinary task promotes the parent and creates the link in one core transaction. The TUI requires confirmation before entering that child flow, and TUI undo restores both promotion and membership. Task creation with an epic parent validates and commits the task and link in one core transaction. The TUI app owns removed-child snapshots used for reversible detail interaction, while renderers receive those snapshots as view-model input.
- Keep portable data-safety validation aligned with recurrence aggregate validation. Recurrence-bearing imports accept only canonical anchored interval rules, IANA zones, stable series and task IDs, lattice slots, deterministic projection task and change identities, deterministic creation timestamps and field-version seeds, status and outcome agreement, nonoverlapping pause intervals, valid lifecycle boundaries, and one projection per series. Archived occurrence tasks and their local notes and attachments remain task rows. Older task-only exports create no recurrence linkage.
- Treat recurrence projection gaps as repairable derived-state gaps. `aven doctor --integrity` directs users to reconcile them through `aven recur list`. Identity, deterministic materialization, outcome, pause, and lifecycle failures require recovery from a known-good backup or careful export of unaffected data.
- Keep shared operation contracts aligned across `crates/aven-core/src/sync/wire.rs`, `crates/aven-core/src/sync/protocol.rs`, `crates/aven-core/src/sync/apply/`, `crates/aven-core/src/sync/encrypted_tail/domain.rs` and the shared-state publication codec. Encrypted tail pages validate operations at replica protocol 18. Reqwest and Axum remain application transport. Recurrence sync operations are compound aggregate writes with canonical schedule, slot, task, change, timestamp, link, and field-version identities. Duplicate change IDs require canonical equality. Outcome conflicts target `recurrence_series` fields named `outcome:YYYY-MM-DD`, lifecycle conflicts target `state`, and only lifecycle conflicts block reconciliation.
- Keep cross-process host exclusion in `src/sync/coordination.rs`. File-backed `Database` values retain a canonical path identity resolved during open. Interactive and TUI sync hold one persistent sidecar guard across their complete drain loop. Daemon sync holds one nonblocking guard across a bounded round and treats contention as deferred scheduling. Never unlink the sidecar because its inode coordinates cooperating Aven processes. In-memory databases and non-sync SQLite writers remain outside this lock, and stale cursor validation remains the transactional correctness backstop.
- Keep bounded sync limits explicit: `ROUND_LIMIT` and `IMAGE_RETRY_ROUNDS` in `src/sync/encrypted.rs` bound an interactive drain, `DAEMON_ROUND_BUDGET` bounds daemon work per wake, the encrypted tail codecs bound page and image bytes, and attachment lifecycle policy bounds prune batches.
- Keep cursor semantics based on `server_seq`. Pull pages are ordered by increasing `server_seq`; response cursors equal the last returned `server_seq` or the request cursor for an empty page; local `sync_cursor` advances only after a validated page applies successfully.
- Keep daemon sync privacy-safe and budget-aware. `src/daemon.rs` runs `sync::encrypted::daemon_round` with `DAEMON_ROUND_BUDGET` rounds per wake; logs and stdout include rounds and metadata and image state without user content. It reschedules promptly only while images transfer or metadata can advance, and an unset database waits at the sync interval without contacting a server.
- Route successful local mutation wake attempts through `daemon::wake_if_enabled`, which owns the sync-enabled condition, wake-address resolution, and wake logging. The daemon loop preserves failure retry deadlines across wakes and uses exponential backoff for failures. Successful sync restores immediate wake-driven scheduling.


## Change routing

| Change | Start here | Also check | Tests |
| --- | --- | --- | --- |
| Add or change the reusable core consumer API | `crates/aven-core/src/api.rs` | underlying core operations, queries, refs, shared IDs and choices, and error translation | `crates/aven-core/tests/consumer_api.rs` and focused `aven-core` tests |
| Add or change device invitation QR construction or CLI presentation | `src/pairing.rs` | `src/sync/encrypted.rs` invite, `src/sync/encrypted/invitation.rs` | `cargo test --lib 'pairing::tests::'` and `cargo test --lib 'sync::encrypted::'` |
| Add or change TUI device invitation presentation | `src/tui/ui/overlays/pairing.rs`, `src/tui/app_pairing.rs` | `src/tui/overlay/state.rs`, `src/tui/overlay/view.rs`, standard modal routing in `src/tui/overlay/mouse.rs`, loop polling in `src/tui/app_lifecycle.rs`, and command scope in `src/tui/event/catalog.rs` and `src/tui/event/action.rs` | `cargo test --lib 'tui::ui::overlays::tests::pairing_overlay::'`, `cargo test --lib 'tui::overlay::mouse::tests::pairing_'`, and `cargo test --lib 'tui::app::tests::command_and_config_overlays::pairing::'` |
| Add or change a CLI command | the matching argument family under `src/cli/`, `src/cli.rs` for the `Commands` variant, `src/lib.rs`, the relevant `src/commands/` family module | `src/cli/help.rs` sections and examples, `src/commands.rs` facade exports, `src/operations/` for writes, `src/input.rs` for text input, `src/render.rs` for shared output helpers, `src/task_render.rs` for task output | focused `tests/cli_*.rs` |
| Change task detail or context output | `crates/aven-core/src/query/details.rs`, `src/commands/context.rs`, the owning renderer under `src/task_render/` (`text.rs`, `json.rs`, `markdown.rs`, `attachments.rs`) | `crates/aven-core/src/refs.rs`, `crates/aven-core/src/query/dependencies.rs`, `src/commands.rs` exports | focused context and show CLI tests, query detail tests, or `cargo check` |
| Add a task scalar field | core migration, `crates/aven-core/src/types.rs`, `crates/aven-core/src/task_fields.rs`, `crates/aven-core/src/mutation.rs` | `crates/aven-core/src/operations/tasks/mutation.rs`, `crates/aven-core/src/db/rows.rs`, `crates/aven-core/src/sync/apply/task.rs`, `crates/aven-core/src/sync/apply/conflict.rs`, `crates/aven-core/src/sync/wire/changes.rs`, core queries, CLI and TUI renderers | sync, conflict, CLI, and TUI tests |
| Add or change task metadata | `crates/aven-core/src/metadata/fields.rs` for field identity and `crates/aven-core/src/metadata/values.rs` for values, the metadata migration, `src/commands/metadata.rs`, `src/commands/tasks.rs`, `src/tui/app_metadata.rs`, `src/tui/overlay/metadata.rs` | task and recurrence operations, `crates/aven-core/src/sync/apply/metadata.rs`, wire validation, undo, search, data safety, consumer API, | focused metadata, recurrence, sync, undo, CLI, consumer API, and data-safety tests |
| Add or change recurrence CLI parsing, commands, context, or rendering | `src/commands/recurrence.rs`, `src/cli/recurrence.rs`, `src/task_render/` | `src/lib.rs`, `src/command_metadata.rs`, `src/commands/context.rs`, core recurrence operations and reports, and `src/skill.md` | `cargo test --test cli_recurrence` plus directly affected CLI tests |
| Add or change recurrence schedule math, deterministic occurrence identity, aggregate behavior, or recurrence reports | `crates/aven-core/src/recurrence/` for schedule math, `crates/aven-core/src/operations/recurrence/` for aggregate behavior (`template.rs`, `projection.rs`, `lifecycle.rs`, `resolution.rs`), `crates/aven-core/src/query/recurrence.rs` | `crates/aven-core/src/recurrence.rs`, `crates/aven-core/src/mutation.rs`, `crates/aven-core/src/operations/tasks/mutation.rs`, `crates/aven-core/src/query/tasks.rs`, `crates/aven-core/src/query/search.rs`, `crates/aven-core/src/query/sidebar.rs`, `crates/aven-core/src/query/recent_actions.rs`, `crates/aven-core/src/undo.rs`, and sync validation when persisted behavior is involved | `cargo test -p aven-core operations::recurrence::tests` for aggregate behavior, `cargo test -p aven-core query::recurrence::tests` for reports, and `cargo test -p aven-core recurrence` for schedule and identity behavior |
| Add symmetric related-task links | `crates/aven-core/src/operations/related.rs`, `crates/aven-core/src/query/related.rs` | sync apply and wire validation, undo and physical deletion guards, data safety, CLI and TUI relationship surfaces, consumer API, | focused related-link, sync, CLI, TUI, export, and consumer tests |
| Add task dependency relations | `crates/aven-core/src/operations/dependencies.rs`, `crates/aven-core/src/query/dependencies.rs` | `src/commands.rs`, `crates/aven-core/src/task_enrichment/dependencies.rs`, `src/task_render/`, `crates/aven-core/src/sync/apply/dependency.rs`, `crates/aven-core/src/sync/encrypted_tail/dependencies.rs` | `tests/cli_dependencies.rs`, `encrypted_tail_http::tests::dependencies::` |
| Add or change image attachments | `crates/aven-core/src/attachments/`, `crates/aven-core/src/operations/attachments.rs`, `crates/aven-core/src/operations/tasks/attachment_creation.rs` | `crates/aven-core/src/task_enrichment/attachments.rs`, `crates/aven-core/src/data_safety/integrity/attachments.rs`, `crates/aven-core/src/sync/`, `src/commands/attachments.rs`, `src/attachments/preview.rs`, `src/attachments/export.rs`, `src/tui/store/attachments.rs`, `src/sync/` | focused core attachment tests, `tests/cli_local.rs`, `tests/cli_attachment_lifecycle.rs`, `encrypted_tail_http::tests::attachments::`, and task creation tests |
| Add or change TUI launch targets | `src/cli/tui.rs`, `src/tui/store/launch.rs`, `src/lib.rs` | `src/tui/store.rs`, `src/tui/mod.rs`, `src/tui/app.rs`, TUI guide and command reference | CLI parser tests, launch resolution tests, focused app startup tests |
| Add or change epic membership | `crates/aven-core/src/operations/epics.rs`, `crates/aven-core/src/task_enrichment/epics.rs`, `crates/aven-core/src/query/tasks.rs` | `src/commands.rs`, `src/tui/store/epics.rs`, `src/tui/ui/task_list/`, `crates/aven-core/src/sync/apply/epic.rs`, `src/skill.md` | `tests/cli_epics.rs`, TUI store tests, query tests, sync tests |
| Change task list, filters, sorting, search read model, or refs | `crates/aven-core/src/query/`, `crates/aven-core/src/query.rs`, `crates/aven-core/src/refs.rs`, `crates/aven-core/src/queue.rs` | CLI list and search rendering, `src/tui/store/types.rs`, `src/tui/store/view.rs`, indexes | query unit tests, `tests/sqlite_read_path_indexes.rs`, focused CLI tests |
| Change TUI task-list rendering or hit testing | `src/tui/ui/task_list/table.rs`, `src/tui/ui/task_list/cells.rs`, `src/tui/ui/task_list/sizing.rs`, `src/tui/ui/task_list/layout.rs`, `src/tui/ui/task_list/hit_test.rs`, `src/tui/ui/task_list/view_model.rs`, `src/tui/ui/task_list/preview.rs` | `src/tui/ui/task_list.rs` facade, `src/tui/ui/empty_state.rs`, `src/tui/store/view.rs`, `src/tui/store/types.rs`, task display helpers, mouse event dispatch | focused owning-module tests, `src/tui/ui/task_list/tests.rs` and its leaf modules, `src/tui/app_tests/task_row_mouse.rs`, focused TUI tests |
| Change TUI detail lifecycle, rendering, or interaction projection | `src/tui/detail_session.rs`, `src/tui/app_detail.rs`, `src/tui/app_detail_input.rs`, `src/tui/ui/detail.rs` facade and `src/tui/ui/detail/` children (`DetailDocument`, `DetailRenderContext`, and section renderers) | `src/tui/app.rs`, `src/tui/ui.rs`, `src/tui/input/key.rs`, `src/tui/input/mouse.rs`, `src/tui/app_mouse.rs`, `src/tui/navigation.rs`, inline-image and selection state | `cargo test --lib 'tui::detail_session::tests::'`, `cargo test --lib 'tui::app::tests::detail_mode::navigation::'`, `cargo test --lib 'tui::app::tests::detail_mode::history_navigation::'`, `cargo test --lib 'tui::app::tests::detail_mode::interaction::'`, and `cargo test --lib 'tui::ui::detail::tests::'` |
| Change configurable TUI board lanes | `src/config/tui.rs`, `src/tui/columns.rs`, `src/tui/ui/columns.rs` | `src/tui/store/types.rs`, `src/tui/app_navigation.rs`, view commands, sidebar, header, mouse dispatch | config tests, `src/tui/columns.rs` tests, column UI tests, focused app tests |
| Add or change the TUI recent actions view | `crates/aven-core/src/query/recent_actions.rs`, `src/tui/ui/recent_actions.rs` | `crates/aven-core/src/change_log.rs` operation names, `src/tui/store.rs`, `src/tui/store/sidebar.rs`, view commands in `src/tui/event/catalog.rs` | `cargo check`, focused TUI store or app tests |
| Change TUI search flow | `src/tui/app_search.rs` | `src/tui/app_overlay_input.rs`, `src/tui/overlay/`, `src/tui/ui/overlays/search.rs`, `crates/aven-core/src/query/` search helpers | `cargo test --lib 'tui::app::tests::command_and_config_overlays::search::'` plus focused query tests |
| Add or change the TUI add-task composer | `src/tui/overlay/state/authoring.rs`, `src/tui/overlay/handlers.rs`, `src/tui/ui/overlays/add_task.rs`, `src/schedule_input.rs` | `src/tui/app_authoring.rs`, `src/tui/app_overlay_input.rs`, picker and label child controls, `src/tui/natural_add_runtime.rs` | `cargo test --lib 'tui::app::tests::authoring::'` and `cargo test --lib 'tui::ui::overlays::tests::add_task_overlay::'` |
| Add or change TUI onboarding | `src/tui/app_onboarding.rs`, `src/tui/store/onboarding.rs`, `crates/aven-core/src/local_state.rs` | timeline scheduling in `src/tui/app_lifecycle.rs`, splash rendering in `src/tui/ui/splash.rs`, `src/tui/overlay/`, `src/tui/ui/overlays/onboarding.rs`, welcome command in `src/tui/event/catalog.rs` | focused app, store, overlay, splash rendering, and welcome rendering tests |
| Add or change a TUI action | `src/tui/event/catalog.rs`, `src/tui/input/action.rs`, focused `src/tui/app_*.rs` modules | key and mouse routes under `src/tui/input/`, flow helpers, overlays, store module, core operation and undo mode, `src/tui/natural_add_runtime.rs` for natural-add worker setup | `cargo test --lib 'tui::event::tests::'`, `cargo test --lib 'tui::app::tests::keyboard_dispatch::'`, and focused app and store leaf modules |
| Change TUI key or mouse routing | `src/tui/input/key.rs`, `src/tui/input/mouse.rs` | `src/tui/event/` command catalog, stable UI view models and hit testing, `src/tui/app_dispatch.rs`, `src/tui/app_mouse.rs`, `src/tui/app_overlay_input.rs`, overlay and detail feature events | `cargo test --lib 'tui::input::key::tests::'`, `cargo test --lib 'tui::input::mouse::tests::'`, `cargo test --lib 'tui::app::tests::keyboard_dispatch::'`, and the affected overlay or detail leaf module |
| Change TUI list selection, marks, browse focus, sidebar visibility, or navigation return state | `src/tui/list_surface.rs` | `src/tui/app_navigation.rs`, `src/tui/app_edit.rs`, `src/tui/input/mouse.rs`, list and sidebar renderers | `cargo test --lib 'tui::list_surface::tests::'` and `cargo test --lib 'tui::app::tests::navigation::'`, plus the owning application leaf module for marks, filters, workspaces, or return state |
| Add or change custom TUI command planning, execution, or completion | `src/tui/custom_command.rs`, `src/tui/app_custom_commands.rs`, `src/tui/app_commands.rs` | `src/tui/custom_command_runtime.rs` for wait and background with output excerpts in `custom_command_runtime/diagnostics.rs`, `src/tui/terminal_command.rs` and `src/tui/platform/terminal.rs` for terminal execution, `src/config/custom_commands.rs` for config validation, command catalog targeting, and application lifecycle ordering | focused planner, runtime, terminal lifecycle, config, and `tui::app::tests::custom_commands::` tests |
| Change terminal inline-image placement, cleanup, or external export retention | `src/tui/inline_image_surface.rs` | `src/tui/app_lifecycle.rs`, `src/tui/preview_controller.rs`, `src/tui/app_attachments.rs`, `src/attachments/export.rs`, terminal backend escapes | `cargo test --lib 'tui::inline_image_surface::tests::'` plus `cargo test --lib 'tui::app::tests::detail_mode::attachments::'`, `cargo test --lib 'tui::app::tests::attachment_save::'`, or `cargo test --lib 'tui::app::tests::attachment_paste::'` for the affected application flow |
| Change TUI task selection or mutation routing | `src/tui/task_selection.rs`, `src/tui/app_edit.rs`, `src/tui/store/task_commands.rs` | action value types, edit-flow capture, row restoration, preserve-visible policy, mutation messages, core structured reports | `cargo test --lib 'tui::task_selection::tests::'` plus the affected leaf command: `cargo test --lib 'tui::store::tests::task_creation_and_updates::creation::'`, `cargo test --lib 'tui::store::tests::task_creation_and_updates::field_updates::'`, `cargo test --lib 'tui::store::tests::task_creation_and_updates::batch_mutations::'`, `cargo test --lib 'tui::store::tests::task_creation_and_updates::notes::'`, `cargo test --lib 'tui::store::tests::task_creation_and_updates::refresh::'`, `cargo test --lib 'tui::store::tests::task_creation_and_updates::rollback::'`, or `cargo test --lib 'tui::store::tests::task_creation_and_updates::selection_restoration::'` |
| Add or change a persisted TUI mutation | owning module under `crates/aven-core/src/operations/` | `crates/aven-core/src/undo.rs`, the focused `src/tui/store/` adapter, mutation report consumers, CLI and reusable no-undo callers | focused core operation test plus the affected fully qualified `tui::store::tests::task_creation_and_updates::<leaf>::` filter listed above, including undo-write failure, no-op, conflict, single-target, and batch cases |
| Change CLI or TUI update behavior | `src/update/`, `src/commands/self_update.rs`, `src/tui/app_update.rs` | `src/lib.rs`, `src/tui/app_lifecycle.rs`, `src/tui/overlay/`, `src/tui/ui/overlays/update.rs`, `src/tui/ui/header.rs`, release artifact names in `.github/workflows/release.yml` | updater unit tests, CLI surface checks, focused app, overlay, and header tests |
| Change the in-TUI changelog reader | `src/tui/changelog.rs`, GitHub default-branch and release-tag `CHANGELOG.md` with one source line per entry | `src/tui/app_update.rs`, `src/tui/app_lifecycle.rs`, `src/tui/event/catalog.rs`, `src/tui/overlay/`, `src/tui/ui/overlays/changelog.rs`, `src/tui/ui/header.rs`, `src/tui/platform/viewer.rs` | `cargo test --lib 'tui::changelog::tests::'` plus focused overlay, header, and platform tests |
| Add or change TUI overlay behavior | the owning family under `src/tui/overlay/state/` (`authoring.rs`, `editors.rs`, `command_search.rs`), `src/tui/overlay/view.rs`, `src/tui/app_overlay_submit.rs` | typed intent payload, input helper, state builder, presentation-kind projection, exhaustive submit dispatch, cancellation, module-local tests | `cargo test --lib 'tui::overlay::'` |
| Add or change TUI overlay rendering | `src/tui/ui/overlays.rs`, `src/tui/ui/overlays/` | overlay view models, shared dialog helpers, input helpers, theme | the owning feature module under `src/tui/ui/overlays/tests/` |
| Change shared operation contracts, host coordination, shared-state bootstrap, or conflict handling | `crates/aven-core/src/db.rs`, `crates/aven-core/src/sync/shared_state.rs`, `crates/aven-core/src/sync/wire/` validation, `crates/aven-core/src/sync/apply/`, `crates/aven-core/src/sync/encrypted_tail/`, `src/sync/coordination.rs` | `src/sync/encrypted.rs`, `src/encrypted_tail_http.rs`, `src/daemon.rs`, core consumer mappings, core mutation and field helpers, data-safety scanners and validators, migrations if persisted | focused shared-state module tests, `encrypted_tail_http::tests::`, root coordination tests, and `cargo test --lib 'sync::encrypted::'` for CLI, conflict and daemon journeys |
| Change seed genesis, first claim, signed initial publication, or protected authority | `crates/aven-core/src/sync/seed_claim.rs`, its `codec.rs` and `persistence.rs`, `src/protected_local_keys/seed.rs` | protected package ownership in `src/protected_local_keys.rs`, frozen bootstrap membership context, secret exclusion and same-path replacement | `cargo test -p aven-core --lib 'sync::seed_claim::tests::'`, `cargo test --lib 'protected_local_keys::'`, `cargo test -p aven-core --lib 'sync::shared_state::package::'`; isolated Keychain tests are explicitly ignored platform checks |
| Change authenticated bootstrap staging | `crates/aven-core/src/sync/bootstrap_staging.rs`, `bootstrap_staging/persistence.rs` | `bootstrap_staging/persistence/publication.rs` current-head authorization and atomic READY, `seed_claim/publication.rs` signed profile, `shared_state/package/publication/staging.rs` structural views, staging/publication migrations, and exact local package ownership | `cargo test -p aven-core --lib 'sync::bootstrap_staging::tests::'`, `cargo test -p aven-core --lib 'sync::seed_claim::'`, `cargo test -p aven-core --lib 'sync::shared_state::package::'`, and `cargo test --lib 'protected_local_keys::seed::tests::publication::'` |
| Change seed bootstrap HTTP transport | `src/seed_bootstrap_http.rs`, `src/seed_bootstrap_http/tests.rs` | protected source/intent and `Database::seed_publication_upload`, core staging and publication codecs | `cargo test --lib 'seed_bootstrap_http::tests::'`, `cargo test --lib 'protected_local_keys::adoption::tests::'`, focused core staging/seed tests, and installation concurrency tests |
| Change pure signed membership and generation codecs | `crates/aven-core/src/sync/seed_claim/membership.rs`, `membership/{encoding,admission,pairing,rotation,keys,evidence}.rs` | unchanged genesis/publication validators, shared pairing primitives in `seed_claim/peer.rs`, `seed_claim/fixtures/{membership,rotation}.json`; the pure API has no storage, network, expiry-clock or dispatch authority | `cargo test -p aven-core --lib 'sync::seed_claim::membership::tests::'` and unchanged seed/publication codec regressions |
| Change repeatable device enrollment and authenticated refresh | `crates/aven-core/src/sync/seed_claim/membership/persistence.rs`, `src/protected_local_keys/{peer,membership}.rs`, `src/peer_enrollment_http.rs` | current-head authorization in bootstrap publication, protected source/adoption, fresh-target and replacement fences, data-only export | `cargo test -p aven-core --lib 'sync::seed_claim::membership::'`, `cargo test --lib 'peer_enrollment_http::tests::'`, seed HTTP/protected adoption tests |
| Change published snapshot reads or verified fresh-peer installation | `src/peer_enrollment_http.rs`, `src/protected_local_keys/peer.rs`, `crates/aven-core/src/sync/shared_state/peer_install.rs` | current membership resolver, `package/publication/download.rs`, shared-state allowlist, attachment storage and transaction-bound cleanup | `cargo test --lib 'peer_enrollment_http::tests::install::'`, shared-state module, protected adoption and seed HTTP tests |
| Change sync commands, daemon or TUI sync, or server launch | `src/sync/encrypted.rs`, `src/sync/encrypted/invitation.rs`, `src/sync/server.rs` | `src/cli/sync.rs`, `src/lib.rs` sync dispatch, `src/daemon.rs`, `src/tui/sync_controller.rs`, protected association helpers in `src/protected_local_keys/peer.rs`, server setup persistence in `crates/aven-core/src/sync/seed_claim/persistence.rs` | `cargo test --lib 'sync::encrypted::'`, `cargo test -p aven-core --lib 'sync::seed_claim::tests::'` |
| Change internal ordinary encrypted task sync | `crates/aven-core/src/sync/encrypted_tail/`, `src/encrypted_tail_http.rs` | protected peer/adoption readiness, current chain resolver, shared parent reducer, domain apply and canonical equality, history ownership triggers | `cargo test --lib 'encrypted_tail_http::tests::'`, `cargo test -p aven-core --lib 'sync::encrypted_tail::tests::'`, parent/apply tests and bootstrap/enrollment/plaintext attachment regressions |
| Add or change backup, export, or import commands | `src/cli/data_safety.rs`, `src/lib.rs`, `src/commands/data_safety/mod.rs`, `crates/aven-core/src/data_safety.rs`, `crates/aven-core/src/data_safety/` | the portable schema in `crates/aven-core/src/data_safety/export_types.rs`, export-payload rules under `crates/aven-core/src/data_safety/validation/`, live checks under `crates/aven-core/src/data_safety/integrity/`, core database open and migration behavior in `crates/aven-core/src/db.rs`, recurrence identity and schedule validation in `crates/aven-core/src/recurrence/`, doctor presentation in `src/commands/doctor/` | `tests/cli_data_safety.rs`, `tests/cli_doctor.rs` |
| Change config, workspace, or project path routing | `src/config/paths.rs`, `src/config.rs`, `src/config_edit.rs`, `src/workspaces.rs`, `src/projects.rs` | config text writes, managed-entry edits, doctor, project commands, TUI workspace and project pickers | `tests/cli_config_daemon.rs`, `tests/cli_workspaces.rs`, `tests/cli_doctor.rs` |
| Change natural-language task intake or agent primer | `src/task_intake.rs`, `src/recurrence_input.rs`, `src/skill.md`, `src/commands/skill.rs` | config schema, `aven prime`, add-task flows, `src/tui/app_intake.rs` for TUI intake state and lifecycle, `src/tui/natural_add_runtime.rs` for TUI background worker setup | `tests/cli_task_intake.rs`, `tests/cli_skill.rs`, focused add-task tests |
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

### Add a TUI overlay intent

1. Add a variant to the intent enum for the owning input shape in `src/tui/overlay/state.rs`.
2. Put every value needed for submission, retry, cancellation, and underlay restoration in the variant.
3. Add the exhaustive submit branch in `src/tui/app_overlay_submit.rs`.
4. Map the intent to a payload-free presentation kind in `src/tui/overlay/view.rs` when rendering differs.
5. Add focused state, handler, submission, cancellation, and rendering tests. Include construction-level coverage when the intent shape makes invalid payload combinations unrepresentable.

### Change sync compatibility

Read [SYNC_PROTOCOL.md](SYNC_PROTOCOL.md) before changing shared operation contracts
or protocol constants. It owns the version-addition checklist, baseline retirement
rules, encoding, standalone/import policy, and required compatibility evidence.

- Preserve historical operation meanings and pending identities. Register new
  operations and closed values without widening the frozen baseline contract.
- Keep local creation, restored history, outgoing/incoming pages, and server
  admission aligned. Core protocol policy lives in `sync/protocol.rs`.
- Never bump `MAINTAINED_PROTOCOL_BASELINE` automatically with the active version.
  Retirement needs explicit authorization and a supported transition for existing
  databases; changing the constant alone can prevent them from opening.
- Retain unchanged released-server interoperability and historical replay proof,
  not just tests with illustrative future protocol numbers.

### Change bounded sync behavior

1. Keep round bounds in `src/sync/encrypted.rs` (`drain`, `ROUND_LIMIT`, `IMAGE_RETRY_ROUNDS`) shared by CLI, TUI and daemon; do not add a second client loop.
2. Keep per-round page, image and byte bounds in `src/encrypted_tail_http.rs` and `crates/aven-core/src/sync/encrypted_tail/`.
3. Keep remote apply in `crates/aven-core/src/sync/apply/` transaction-safe and cursor-safe.
4. Keep daemon work in `src/daemon.rs` bounded by `DAEMON_ROUND_BUDGET`, rescheduling promptly only while `Outcome::more_work_ready` holds.
5. Use focused validation commands: `cargo test --lib 'sync::encrypted::'` and the affected `encrypted_tail_http::tests::` leaf.

### Add a TUI action

1. Add an `Action` variant and register it in the command catalog under `src/tui/event/`.
2. Add the exhaustive execution branch in `src/tui/input/action.rs` and delegate feature behavior to its focused application owner.
3. Add or reuse overlay state with the typed intent and complete submission payload.
4. Add flow helpers only for state that lives outside an active overlay or search intent.
5. Add `TuiStore` facade methods and focused store logic.
6. For persisted mutations, request TUI undo from the owning core operation and consume its structured report. Keep CLI and reusable callers on the explicit no-undo operation path.
7. Add tests for shortcut resolution, action dispatch, intent payload propagation, cancellation cleanup, core transaction rollback, and store behavior.

## Development and validation

`scripts/process-lock` owns token-safe generic process locking for hook stash
coordination. It uses native `lockf` on macOS and util-linux `flock` on Linux;
`scripts/test-process-lock` covers owner death, stale tokens, contention, and
cleanup. `scripts/test-pre-commit` exercises stash preservation across worktrees.
The same generic helper can be maintained as a local copy by downstream consumers.
No helper requires a mobile checkout.


Use `just` as the main development entrypoint:

- `just check`: local read-only validation gate, equivalent to `just pre-commit`.
- `just test`: Rust test suite through `cargo nextest`, plus Rust doctests.
- `just migration-new <lower_snake_name>`: create the next SQLx migration filename safely.
- `just sqlx-prepare`: regenerate SQLx offline query metadata after migrations or query shape changes.
- `just sqlx-check`: verify SQLx offline query metadata.
- `just run -- ...`: run the application.

The pre-commit hook runs the fast formatting, static analysis, migration order, and clippy gate. The pre-push hook runs `just check-full`, including tests, doctests, SQLx metadata checks, and redundant compile gates. Local project instructions say formatting and broad tests run automatically through hooks, so run focused commands while developing and let the hooks run their configured gates when committing and pushing.

Cargo test filters are substring matches. For TUI unit tests, use the fully qualified module path ending in `::`, for example `cargo test --lib 'tui::app::tests::navigation::'`, so similarly named leaf modules do not expand the scope. When targeting one test in a split family, include every leaf component, such as `cargo test --lib 'tui::app::tests::detail_mode::navigation::detail_next_and_previous_task_stay_in_detail'`. The module path follows the Rust declarations rather than the test file path: app fixtures under `src/tui/app_tests/` live below `tui::app::tests::`, while store fixtures under `src/tui/store/tests/` live below `tui::store::tests::`.

TUI detail lifecycle changes use the affected leaf filter under `tui::app::tests::detail_mode::`: `attachments::`, `editing::`, `history_navigation::`, `interaction::`, `navigation::`, or `relationships::`. Detail session and rendering tests use `tui::detail_session::tests::` and `tui::ui::detail::tests::`. List surface changes use `cargo test --lib 'tui::list_surface::tests::'` plus `cargo test --lib 'tui::app::tests::navigation::'` for list and sidebar navigation. Detail navigation uses `cargo test --lib 'tui::app::tests::detail_mode::navigation::'`, and detail back-stack behavior uses `cargo test --lib 'tui::app::tests::detail_mode::history_navigation::'`. Command and configuration overlay application tests use the affected leaf below `tui::app::tests::command_and_config_overlays::`: `batch_editing::`, `command_palette::`, `configuration::`, or `search::`. Overlay rendering fixtures use the file-name leaf below `tui::ui::overlays::tests::`, such as `add_task_overlay::` or `picker_overlays::`.

Inline image surface changes use `cargo test --lib 'tui::inline_image_surface::tests::'` plus the affected attachment application leaf. Persisted TUI mutation changes require one focused core operation test and the affected `creation::`, `field_updates::`, `batch_mutations::`, `notes::`, `refresh::`, `rollback::`, or `selection_restoration::` leaf below `tui::store::tests::task_creation_and_updates::`. Cover undo-write rollback and the relevant no-op, conflict, single-target, or batch contract without running the broad suite during iteration.
