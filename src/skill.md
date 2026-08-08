# Aven CLI Primer

`aven` is a local-first task manager backed by SQLite. Use it to find work,
inspect tasks, update status, and leave durable handoff context.

## Start here

- Use `aven doctor` when the active workspace, database, or project routing is
  unclear.
- Run `aven skill install` when the aven skill should be installed into detected
  coding-agent skill directories. Use `--agent <claude|opencode|codex>` to
  choose explicit targets.
- Run `aven <command> --help` when you need flags not shown here.

## Task refs and values

- Use refs printed by command output, preferably qualified refs like `APP-7KQ9`.
- Do not mention task refs in commit messages, PR descriptions, or external
  systems. They identify the user's local tasks.
- The suffix is stable identity. The project prefix is display context and can
  change when a task moves projects.
- Bare suffix refs work when unambiguous.
- Typed suffix refs must be at least 3 characters.
- If `aven` reports `ambiguous-ref`, retry with a longer suffix.
- Status values are `inbox`, `backlog`, `todo`, `active`, `done`, and
  `canceled`. TUI column names group these values for presentation and are not
  valid status values for CLI or agent updates.
- Priority values are `none`, `low`, `medium`, `high`, and `urgent`.
- Add `--workspace <name-or-key>` when a command must target a specific
  workspace.

## Configuration

Use `aven config get <key>` to read `sync.enabled`, `sync.server_url`,
`sync.interval_seconds`, `update.automatic_checks`, `local.db_path`, or
`local.image_optimization`. Optional strings print `null` when unset. Use
`aven config set <key> <value>` to persist a validated scalar, and pass `null` to
clear `sync.server_url` or `local.db_path`. Secret settings such as
`sync.auth_token` are unavailable through these commands.

## Core commands

```sh
aven list --project app
aven list --open
aven list --open --project app --label bug
aven list --status todo
aven list --all
aven list --deleted
aven list --ready
aven list --blocked
aven list --upcoming
aven list --overdue
aven search "auth bug"
aven search --project app --metadata legacy-id=123 "migration"
aven search --has-metadata customer-id "handoff"
aven context APP-7KQ9
aven show APP-7KQ9 --full
aven add "Fix conflict display" --status todo --priority high --label bug
aven add "Import legacy task" --metadata legacy-id=123
aven edit APP-7KQ9 --metadata customer-id=acme-42
aven edit APP-7KQ9 --remove-metadata customer-id
aven metadata list
aven metadata rename legacy-id source-id
aven add "Test rollout" --available-at tomorrow
aven add "Submit report" --due "next monday"
aven add "Add due dates" --epic
aven add "Draft release notes" --natural --project app
aven epic add APP-7KQ9 APP-7KQ0
aven epic remove APP-7KQ9 APP-7KQ0
aven epic list APP-7KQ0
aven dep add APP-7KQ9 APP-7KQ0
aven dep remove APP-7KQ9 APP-7KQ0
aven dep list APP-7KQ9
aven edit APP-7KQ9 --status active
aven edit APP-7KQ9 --title "Clearer title" --priority medium
aven edit APP-7KQ9 --available-at tomorrow
aven edit APP-7KQ9 --clear-available-at
aven edit APP-7KQ9 --due "in 2 weeks"
aven edit APP-7KQ9 --clear-due
aven update
aven project list --search app
aven label list --search bug
aven note APP-7KQ9 "durable handoff context"
aven attachment add APP-7KQ9 ./diagram.png --alt "architecture diagram"
aven attachment list APP-7KQ9
aven attachment get 7KQ9A1X4MV2P8D6R --output diagram.png
aven attachment delete 7KQ9A1X4MV2P8D6R
aven attachment prune --dry-run
aven attachment prune --apply
aven delete APP-7KQ9
aven restore APP-7KQ9
```

## Recurring tasks

```sh
aven add "Daily journal" --repeat daily --repeat-at 09:00 \
  --repeat-due same-day --time-zone Europe/Stockholm \
  --repeat-start-on 2026-07-20
aven recur list
aven recur show RCR-7KP2
aven recur history RCR-7KP2
aven recur edit RCR-7KP2 --title "Future journal" --priority high
aven recur edit RCR-7KP2 --metadata journal-kind=work
aven recur skip RCR-7KP2
aven recur pause RCR-7KP2
aven recur resume RCR-7KP2
aven recur stop RCR-7KP2
aven recur stop RCR-7KP2 --skip-current
aven list --status done --expand-recurring
aven search "daily journal" --expand-recurring
```

- Series refs use the `RCR-7KP2` form. Use the series ref for recurrence
  commands and durable references. A current or historical occurrence task ref
  also resolves to its series for convenience.
- Recurrence rules use the fixed grammar `daily`, `weekdays`, `weekly`,
  `fortnightly`, `monthly`, `weekly on mon,wed,fri`, `every N weeks`, or `every
  N weeks on mon,thu`. Weekly shorthands use the start date's weekday, and weeks
  begin on Monday. Monthly rules use the start date's day number and clamp to
  the final day of shorter months without changing that anchor. Completion-relative,
  yearly, cron, and bare every-N-day rules are outside this model.
- `--repeat-at` is a local `HH:MM` time in the series IANA time zone.
  `--repeat-due` is `same-day` or `none`. Ordinary `--available-at` and `--due`
  are absolute task fields and cannot be combined with `--repeat`.
- A series projects at most one current ordinary task. Complete the occurrence
  with `edit <task-ref> --status done`, or skip it with `recur skip`. A canceled
  recurrence occurrence renders as skipped on recurrence surfaces.
- Series template edits affect future occurrences. Schedule rule, start date,
  and time zone are fixed for a series. Metadata template edits also affect only
  future materialized occurrences. Stop the series and create a replacement to
  change its calendar lattice.
- Paused occurrences stay addressable by task ref but leave ordinary active
  views. Stopping keeps the current occurrence as the final task unless
  `--skip-current` resolves it immediately.
- Done and search group recurring history by series. Use `--expand-recurring`
  for occurrence task rows and `recur history` for completed, skipped, derived
  missed, archived missed, and paused entries. A past slot without an occurrence
  task stays missed and has no completion or skip mutation.
- List-like reports reconcile a bounded set of recurring series before reading.
  After a scheduled slot boundary, one report can append one bounded projection
  operation per changed series, including archiving one superseded projection
  and materializing one current projection.

- `aven list --open` returns available, live tasks with `inbox`, `backlog`,
  `todo`, or `active` status. Combine it with project, priority, label, readiness,
  blocker, epic, overdue, recurrence-expansion, and limit filters. Use explicit
  status, all, deleted, or upcoming selectors separately.
- `aven add --status <status>` creates already-triaged work atomically. Plain
  tasks default to `inbox`. Recurring tasks default to `todo` and accept only an
  open initial status.
- `aven add --natural --project <project>` fixes the project used by natural
  intake. Natural intake determines title, description, project, priority,
  labels, availability, due date, and recurrence. Ordinary results use `inbox`
  and remain non-epic, while recurring results use `todo`.

- Use `aven <command> --help` to find maintenance commands for renaming,
  deletion, backup, export, import, and integrity checks.

- `aven update` checks for a newer release. Direct installations require
  `aven update --yes` before replacing the executable. Package-managed
  installations print the appropriate update guidance.
- Use `show --full` before decisions that depend on description, labels, metadata,
  notes, deletion state, or conflicts. Metadata keys are workspace definitions:
  renaming one preserves its stable identity and updates how every related value
  is displayed. Empty metadata values are present values, not removal.
- Use `context <ref>` when one task snapshot is needed before acting. It gathers
  task fields, description, labels, notes, dependencies, blockers, conflicts,
  current attachment metadata, deletion state, refs, and project metadata.
- Human-readable CLI output is the default agent-facing format. Use
  `context <ref> --json` only when a machine-readable snapshot is needed for a
  script, MCP server, bot, web UI, or other structured integration.
- After `aven add`, capture and report the printed ref so future agents can use
  it.
- Attachment commands keep bytes outside text and JSON output. Use
  `attachment add <ref> <path>` to store image bytes, `attachment list <ref>
  --json` to inspect current metadata, `attachment list <ref> --all --json` to
  include tombstones, and `attachment get <attachment-id> --output <path>` to
  write live attachment bytes to a file. Attachment IDs are exact
  workspace-scoped identifiers. File output requires a local available blob and
  refuses to overwrite an existing output path. `attachment add` accepts
  `--media-type`, `--filename`, and `--alt`. PNG, JPEG, GIF, and WebP formats are
  detected from decoded content, and Aven derives canonical media type and
  dimensions from the stored bytes. An explicit `--media-type` must match the
  detected format.
- Image optimization is off by default. `local.image_optimization: paste` applies
  lossless PNG optimization to pasted images, while `on` also applies it to CLI
  file attachments. `attachment add --optimize` and `--no-optimize` override the
  configured file-attachment behavior.
- Attachment JSON from `add`, `list`, `get`, and `delete` contains
  `attachment_id`, `task_id`, `sha256`, `byte_size`, `media_type`, `filename`,
  `alt_text`, `width`, `height`, `created_at`, `deleted`, `deleted_at`, and
  `has_blob`. Add output also contains `optimized`. `context --json` and
  `show --full --json` preserve live attachments and deleted tombstones, omit
  `sha256`, and contain the other metadata fields. Structured consumers must
  inspect `deleted` rather than treating every array entry as a current
  attachment. Human-readable task attachment sections contain live attachments
  only. Attachment deletion is an idempotent tombstone operation. There is no
  attachment restore command. Tombstone metadata does not guarantee that image
  bytes remain available: sync does not download deleted attachment blobs, and
  pruning may remove a local blob.
- `attachment prune` reports one privacy-safe batch of eligible counts and bytes,
  capped by `local.attachment_lifecycle.maintenance_limit`. JSON output contains
  `mode`, `eligible_count`, `eligible_bytes`, `pruned_count`, and `pruned_bytes`.
  Repeated runs may be needed. Original objects are deleted only with `--apply`;
  disposable preview cache eviction to `preview_quota_bytes` also runs in dry-run
  mode. Grace periods, live references, unsynced adds, staging, transfers, reads,
  backups, and upload reservations protect original blobs from pruning.
- Local add or download capacity and server workspace upload capacity enforce
  attachment quotas with `error attachment-quota-exceeded`. Sync exits before a
  completion summary on this error. Pulled metadata and cursor progress remain
  committed when a local download is blocked. Local pruning frees only local
  capacity. Increase `quota_bytes` for local capacity or
  `server_workspace_quota_bytes` in the server configuration for server capacity,
  then retry.
- Task detail read surfaces include ordered attachment metadata and `has_blob`
  without embedding bytes. The TUI renders live attachments after the description
  in a dedicated section and uses bounded, device-local previews when the terminal
  backend supports them. Pending downloads and unavailable local bytes use text
  placeholders. `Tab` focuses locally available images in task detail. `Enter`
  opens the large TUI preview when supported and otherwise uses the operating
  system image viewer. `o` opens the focused image, or the current large-preview
  image, in the operating system viewer. Search matches attachment filename and
  alt text, not hashes, sidecar paths, or bytes.
- `aven backup` writes one archive containing the complete SQLite database and
  local attachment objects. Series templates, labels, sparse occurrences, pause
  intervals, conflicts, field versions, archived occurrence tasks, and all
  occurrence-local data remain intact. `aven backup restore <path> --yes`
  restores that archive and keeps a SQLite safety copy.
- `aven export` writes portable JSON containing the recurrence aggregate and
  attachment metadata without image bytes. `aven import --yes <path>` validates
  recurrence schedule, zone, identity, lattice, deterministic projection,
  outcome, pause, lifecycle, and uniqueness invariants before replacing local
  data. Older task-only exports import every task as nonrecurring data. Imported
  blob inventory is unavailable until sync or backup restore supplies the
  bytes.
- `aven doctor --integrity` distinguishes a repairable recurrence projection gap
  from identity, deterministic materialization, outcome, pause, or lifecycle
  corruption. Run `aven recur list` to reconcile a projection gap. Preserve the
  database and restore a known-good backup before addressing corruption.
- `aven doctor` checks attachment metadata and local object presence and reports
  referenced, protected, grace-period, eligible, staging, trash, quota, and
  lifecycle inconsistency counts and bytes. `aven doctor --integrity` also
  decodes original images and compares their format and dimensions with stored
  metadata.
- Task descriptions remain scalar user-authored Markdown text. Attachment adds
  and deletes change attachment metadata without changing the description.
- When creating follow-up tasks from a discussion, investigation, review, or
  plan, include enough detail in the task description for the task to stand
  alone. Capture rationale, scope, acceptance criteria, implementation notes,
  and related tasks when useful. Prefer `--description-stdin` for
  multi-paragraph descriptions. Use `--description-file` when the file already
  exists with the intended content.
- Add dependencies between related tasks when one task must finish before another
  can start. Use `dep add <blocked> <blocker>`.
- Use epics when one task is part of a larger body of work. Create an empty epic
  with `add --epic`, convert a task with `edit <ref> --epic on`, and link
  children with `epic add <child> <epic>`.
- Epic membership does not make a child blocked. Do not use dependencies to model
  epic membership, and do not use epics to model ordering.
- Epics do not nest, and each child belongs to one epic. Work child tasks
  individually and mark the epic done when the larger outcome is complete.
- `list --ready` excludes epics so agents pick actionable child tasks.
- `available_at` defers attention until its timestamp. Set it with
  `add --available-at <when>` or `edit --available-at <when>`, clear it with
  `edit --clear-available-at` or the value `now`, and use `list --upcoming` to
  inspect deferred tasks. It accepts local-calendar dates such as `tomorrow`,
  `2w`, `in 2 months`, and `next mon at 9am`, plus ISO dates, UTC timestamps,
  and Unix timestamps. Do not use availability for deadlines.
- `due_on` is an optional completion deadline on the local calendar. Set it with
  `add --due <when>` or `edit --due <when>`, clear it with `edit --clear-due`
  or the value `none`, and use `list --overdue` to inspect missed deadlines.
  It accepts the same local-calendar date forms as availability, without times.
  Due dates do not hide tasks, change status, clear availability, or create
  notifications. Combine
  `list --upcoming --overdue` to find deferred tasks whose deadlines passed.
- Let commands infer the project from the current directory, even if project
  does not exist yet. Pass `--project` only if project is specified by user.
- Use `project rename <old> <new> [--prefix <prefix>]` when a project itself
  has a wrong name or prefix. Use task updates only when moving tasks between
  distinct projects.
- Use `search <query>` when finding an unknown task by ref, title,
  description, project, label, note, status, or priority. Search includes done
  and canceled tasks in the active workspace.
- Use `list --deleted` with normal filters to list deleted tasks only.
- Use `list --all` with normal filters to include deleted tasks with live tasks.
- Use `search <query> --all` when deleted tasks should be included in broad
  search results. Ref-shaped search input can return a deleted task and prints
  deleted metadata when it does.
- Use `bulk-update --dry-run` before broad mutations.
- Use `list --ready` when selecting new work to avoid blocked or completed tasks.
- In `aven prime`, Active, Ready, and Blocked partition open issues by
  pickability. `blocked_by=[REFS]` lists unresolved blockers, and
  `blocks=[REFS]` lists open dependents. Run `aven show <ref> --full` for
  descriptions, notes, resolved dependencies, and conflicts.
- Inspect dependency context with `show <ref> --full` before changing task order or
  status. The `depends_on` and `blocks` sections show blockers and dependents.

## Structured output

- Human-readable output is the default and preferred for agent use.
- `--json` is available on `context`, `search`, `list`, `show`, `recur list`,
  `recur show`, `recur history`,
  `attachment list`, `attachment get`, `attachment add`, `attachment delete`,
  `attachment prune`, `dep list`,
  `epic list`, `project list`, `label list`, `conflict list`, `conflict show`,
  `prime`, and `doctor`.
- `list`, `show`, and `search` JSON use the same flat task object. Search adds
  `score`, `matched_field`, and `snippet`. Task objects include scheduling, epic,
  dependency, conflict, timestamp, and recurrence state. Recurrence series list,
  detail, and history reports carry `version: 1` and a typed `kind`. An empty
  `available_at` means the task is immediately available. An empty `due_on`
  means the task has no deadline.
- Use `--limit <n>` with list-style reads such as `list`, `project list`,
  `label list`, `conflict list`, and `prime` to bound response size. Explicit
  limits must be positive. `recur history --limit` accepts values from `1` to
  `500`.

## Sync behavior

```sh
aven sync
aven daemon
aven daemon restart
```

- Sync output reports pushed and pulled metadata counts, attachment blob upload and
  download counts and byte totals, remaining blob counts and bytes, a cursor, and
  completion state. Blob counts are unique by content hash and stay separate from
  metadata change counts.
- Each attachment transfer round allows up to 16 unique blobs and 64 MiB across
  uploads and downloads. `complete=false` means metadata or required blob
  transfers remain. Remaining uploads can be prerequisites for unsynced
  attachment-add operations, while downloads are limited to attachments on live
  tasks. Interactive sync continues through bounded rounds while progress is
  possible.
- Sync transfers attachment bytes through authenticated blob endpoints and keeps
  bytes out of `/sync` JSON payloads.
- `aven daemon restart` restarts the macOS LaunchAgent service.

## Long input and secrets

- Use `--description-stdin`, `note --stdin`, or an existing file with
  `--description-file` or `note --file` for long Markdown instead of
  shell-escaping large text.
- Avoid `tmpfile=$(mktemp); cat > "$tmpfile"` in shell snippets. Some agent
  shells enable `noclobber`, which makes `>` fail because `mktemp` creates the
  file before the redirect runs. Use a heredoc directly into
  `--description-stdin`, or write a new path inside `mktemp -d`.
- Notes are append-style and better for durable handoff context than scratch
  work.
- Avoid writing secrets into titles, descriptions, labels, projects, notes, or
  logs.
- Use the safe text workflow for descriptions:
  `aven text get <ref> description --output description.md`
  `aven text diff <ref> description --file description.md`
  `aven text set <ref> description --file description.md --if-sha256 <sha256>`

## Conflicts

```sh
aven conflict list
aven conflict show APP-7KQ9
aven conflict diff APP-7KQ9 description
```

- Inspect conflicts before resolving them.
