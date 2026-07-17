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

## Core commands

```sh
aven list --project app
aven list --status todo
aven list --all
aven list --deleted
aven list --ready
aven list --blocked
aven list --upcoming
aven list --overdue
aven search "auth bug"
aven context APP-7KQ9
aven show APP-7KQ9 --full
aven add "Fix conflict display" --priority high --label bug
aven add "Test rollout" --available-at tomorrow
aven add "Submit report" --due "next monday"
aven add "Add due dates" --epic
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
aven delete APP-7KQ9
aven restore APP-7KQ9
```

- Use `aven <command> --help` to find maintenance commands for renaming,
  deletion, backup, export, import, and integrity checks.

- `aven update` checks for a newer release. Direct installations require
  `aven update --yes` before replacing the executable. Package-managed
  installations print the appropriate update guidance.
- Use `show --full` before decisions that depend on description, labels, notes,
  deletion state, or conflicts.
- Use `context <ref>` when one task snapshot is needed before acting. It gathers
  task fields, description, labels, notes, dependencies, blockers, conflicts,
  deletion state, refs, and project metadata.
- Human-readable CLI output is the default agent-facing format. Use
  `context <ref> --json` only when a machine-readable snapshot is needed for a
  script, MCP server, bot, web UI, or other structured integration.
- After `aven add`, capture and report the printed ref so future agents can use
  it.
- Attachment commands keep bytes outside text and JSON output. Use
  `attachment add <ref> <path>` to store image bytes, `attachment list <ref>` or
  `attachment get <attachment-id>` to inspect metadata, and
  `attachment get <attachment-id> --output <path>` to write bytes to a file.
  File output requires a local available blob and refuses to overwrite an
  existing output path. `attachment add` accepts `--media-type`, `--filename`,
  and `--alt`. PNG, JPEG, GIF, and WebP formats are detected from decoded
  content, and Aven derives canonical media type and dimensions from the stored
  bytes. An explicit `--media-type` must match the detected format.
- Task detail read surfaces include ordered attachment metadata and `has_blob`
  without embedding bytes. The TUI renders live attachments after the description
  in a dedicated section and uses bounded, device-local previews when the terminal
  backend supports them. Pending downloads, unavailable local bytes, and disabled
  previews use text placeholders. Search matches attachment filename and alt text,
  not hashes, sidecar paths, or bytes.
- `aven backup` writes one archive containing SQLite data and local attachment
  objects. `aven backup restore <path> --yes` restores that archive and keeps a
  SQLite safety copy. `aven export` writes attachment metadata and blob
  inventory without bytes, and `aven import --yes <path>` imports that metadata
  with blob inventory marked unavailable.
- `aven doctor` checks attachment metadata and local object presence.
  `aven doctor --integrity` also decodes original images and compares their
  format and dimensions with stored metadata.
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
- `--json` is available on `context`, `search`, `list`, `show`,
  `attachment list`, `attachment get`, `attachment add`, `attachment delete`,
  `dep list`,
  `epic list`, `project list`, `label list`, `conflict list`, `conflict show`,
  `prime`, and `doctor`.
- JSON task objects include `available_at`, `due_on`, `is_epic`, `epic_parent`,
  and `epic_children`. An empty `available_at` means the task is immediately
  available. An empty `due_on` means the task has no deadline. Use the epic
  fields to distinguish epic membership from dependency ordering.
- Use `--limit <n>` with list-style reads such as `list`, `project list`,
  `label list`, `conflict list`, and `prime` to bound response size.

## Sync behavior

```sh
aven sync
aven daemon
aven daemon restart
```

- Sync output reports pushed and pulled counts, attachment blob upload and
  download counts, a cursor, and completion state. Blob counts are separate from
  metadata change counts.
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
