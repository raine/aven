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
aven backup --output .backup/data.sqlite
aven backup restore .backup/data.sqlite --yes
aven export --output snapshot.json
aven import snapshot.json --yes
aven doctor --integrity
```

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
