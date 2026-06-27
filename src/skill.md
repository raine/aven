# Aven CLI Primer

`aven` is a local-first task manager backed by SQLite. Use it to find work,
inspect tasks, update status, and leave durable handoff context.

## Start here

- Use `aven doctor` when the active workspace, database, or project routing is
  unclear.
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
  `canceled`.
- Priority values are `none`, `low`, `medium`, `high`, and `urgent`.
- Add `--workspace <name-or-key>` when a command must target a specific
  workspace.

## Core commands

```sh
aven list --project app
aven list --status todo
aven list --all
aven list --ready
aven list --blocked
aven context APP-7KQ9
aven show APP-7KQ9 --full
aven add "fix conflict display" --priority high --label bug
aven dep add APP-7KQ9 APP-7KQ0
aven dep remove APP-7KQ9 APP-7KQ0
aven dep list APP-7KQ9
aven update APP-7KQ9 --status active
aven update APP-7KQ9 --title "clearer title" --priority medium
aven project list --search app
aven project rename old-project "New Project Name" --prefix NPN
aven label list --search bug
aven note APP-7KQ9 "durable handoff context"
aven delete APP-7KQ9
aven restore APP-7KQ9
aven project delete old-project
aven label delete obsolete
aven note-delete APP-7KQ9 0123456789ABCDEF
aven backup --output .backup/data.sqlite
aven backup restore .backup/data.sqlite --yes
aven export --output snapshot.json
aven import snapshot.json --yes
aven doctor --integrity
```

- Use `show --full` before decisions that depend on description, labels, notes,
  deletion state, or conflicts.
- `aven prime` includes local convention summaries for the inferred or requested
  project, such as sampled title style, statuses, and labels.
- Use `context <ref>` when one task snapshot is needed before acting. It gathers
  task fields, description, labels, notes, dependencies, blockers, conflicts,
  deletion state, refs, and project metadata.
- Human-readable CLI output is the default agent-facing format. Use
  `context <ref> --json` only when a machine-readable snapshot is needed for a
  script, MCP server, bot, web UI, or other structured integration.
- After `aven add`, capture and report the printed ref so future agents can use
  it.
- Let commands infer the project from the current directory, even if project
  does not exist yet. Pass `--project` only if project is specified by user.
- Use `project rename <old> <new> [--prefix <prefix>]` when a project itself
  has a wrong name or prefix. Use task updates only when moving tasks between
  distinct projects.
- Use `list --all` with normal filters to find deleted tasks.
- Use `bulk-update --dry-run` before broad mutations.
- Use `list --ready` when selecting new work to avoid blocked or completed tasks.
- Inspect dependency context with `show <ref> --full` before changing task order or
  status. The `depends_on` and `blocks` sections show blockers and dependents.
- Use `dep add <blocked> <blocker>` when one task depends on another.

## Sync behavior

```sh
aven sync
aven sync --server http://127.0.0.1:3000
aven daemon
```

- `aven sync` drains bounded push and pull pages until local unsynced changes and
  remote pages are complete.
- Sync pins a database to its server URL. Use a fresh database for a different
  sync server.
- Sync output reports `synced pushed=<n> pulled=<n> cursor=<server_seq>`. The
  cursor is based on `server_seq` and advances after validated pages apply.
- The sync client pushes at most 256 local changes per page and requests at most
  512 remote changes per pull page.
- The sync server validates protocol version, request cursor, push batch size,
  pull limit, operation names, entity types, and payload shapes before accepting
  changes.
- Push acknowledgements match pushed change IDs. Duplicate pushed change IDs keep
  their existing `server_seq` values.
- Pull pages are ordered by increasing `server_seq`; `has_more` means another
  bounded pull page is available.
- The daemon sync path processes a fixed page budget per wake. Incomplete daemon
  rounds print `daemon-synced pushed=<n> pulled=<n> cursor=<server_seq>
  complete=false pages=<n>` and schedule follow-up sync work.
- Sync logs and daemon sync output carry counts, cursor, completion, and page
  count. They do not include task titles, descriptions, note bodies, labels,
  project names, auth tokens, or raw payloads.
- Use `project delete`, `label delete`, and `note-delete` for synced project,
  label, and note deletion. Their sync logs use counts and IDs rather than
  user-authored names or note bodies.

Focused validation commands:

```sh
cargo test --test cli_sync sync_server_returns_bounded_pull_pages
cargo test --test cli_sync sync_client_drains_paged_remote_changes
cargo test --test cli_sync sync_client_drains_paged_local_changes
cargo test --test cli_sync current_protocol_version_sync_succeeds
cargo test --test cli_sync wrong_response_protocol_version_is_rejected
cargo test --test cli_daemon_sync daemon_syncs_large_backlog_across_budgeted_rounds
cargo test --test cli_logging daemon_sync_logging_redacts_task_content
```

## Long input and secrets

- Use `--description-file`, `--description-stdin`, `note --file`, or
  `note --stdin` for long Markdown instead of shell-escaping large text.
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
aven conflict resolve APP-7KQ9 description --use <variant-token>
aven conflict resolve APP-7KQ9 description --value-file value.md
aven conflict diff APP-7KQ9 description
aven conflict export APP-7KQ9 description --dir ./conflict-variants
```

- Inspect conflicts before resolving them.
- Use `conflict show` to see stable variant tokens.
- Resolve only after selecting the intended value.
- Do not bulk resolve conflicts or default to newest, local, or remote.
