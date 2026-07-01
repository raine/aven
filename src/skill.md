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
aven list --deleted
aven list --ready
aven list --blocked
aven search "auth bug"
aven context APP-7KQ9
aven show APP-7KQ9 --full
aven add "fix conflict display" --priority high --label bug
aven add "add due dates" --epic
aven epic add APP-7KQ9 APP-7KQ0
aven epic remove APP-7KQ9 APP-7KQ0
aven epic list APP-7KQ0
aven dep add APP-7KQ9 APP-7KQ0
aven dep remove APP-7KQ9 APP-7KQ0
aven dep list APP-7KQ9
aven update APP-7KQ9 --status active
aven update APP-7KQ9 --title "clearer title" --priority medium
aven project list --search app
aven label list --search bug
aven note APP-7KQ9 "durable handoff context"
aven delete APP-7KQ9
aven restore APP-7KQ9
```

- Use `aven <command> --help` to find maintenance commands for renaming,
  deletion, backup, export, import, and integrity checks.

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
- When creating follow-up tasks from a discussion, investigation, review, or
  plan, include enough detail in the task description for the task to stand
  alone. Capture rationale, scope, acceptance criteria, implementation notes,
  and related tasks when useful. Use `--description-file` or
  `--description-stdin` for multi-paragraph descriptions.
- Add dependencies between related tasks when one task must finish before another
  can start. Use `dep add <blocked> <blocker>`.
- Use epics when one task is part of a larger body of work. Create an empty epic
  with `add --epic`, convert a task with `update <ref> --epic on`, and link
  children with `epic add <child> <epic>`.
- Epic membership does not make a child blocked. Do not use dependencies to model
  epic membership, and do not use epics to model ordering.
- Epics do not nest, and each child belongs to one epic. Work child tasks
  individually and mark the epic done when the larger outcome is complete.
- `list --ready` excludes epics so agents pick actionable child tasks.
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
- `--json` is available on `context`, `search`, `list`, `show`, `dep list`,
  `epic list`, `project list`, `label list`, `conflict list`, `conflict show`,
  `prime`, and `doctor`.
- JSON task objects include `is_epic`, `epic_parent`, and `epic_children`.
  Use these fields to distinguish epic membership from dependency ordering.
- Use `--limit <n>` with list-style reads such as `list`, `project list`,
  `label list`, `conflict list`, and `prime` to bound response size.

## Sync behavior

```sh
aven sync
aven daemon
```

- Sync output reports pushed and pulled counts, a cursor, and completion state.

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
aven conflict diff APP-7KQ9 description
```

- Inspect conflicts before resolving them.
