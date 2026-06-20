# Agentic Task Manager CLI Primer

## Core model

- Binary: `atm`, a local-first task manager backed by SQLite.
- Workspace scope is part of task, project, label, and note lookup.
- Tasks have statuses: `inbox`, `backlog`, `todo`, `active`, `done`, `canceled`.
- Priorities are: `none`, `low`, `medium`, `high`, `urgent`.
- Tasks are soft-deleted with `delete` and can be recovered with `restore`.
- Labels and notes are append-style supporting data. Notes are better for durable handoff context than scratch work.
- Projects normalize to lowercase hyphenated keys and get short display prefixes.

## References

- Command output prints refs like `APP-7KQ9` and also prints a bare `ref=7KQ9` on creation.
- Prefer the printed qualified ref when possible.
- Bare suffix refs work when unambiguous.
- Typed suffix refs must be at least 3 characters.
- If `atm` reports `ambiguous-ref`, retry with a longer suffix.
- The project prefix is display context, not identity. If a task moves projects, the suffix remains stable and the prefix can change.

## Workspace selection

- Use `atm doctor` when the active workspace is unclear.
- Add `--workspace <name-or-key>` when a command must target a specific workspace.
- Project path routes can select the active workspace from the current directory.

## Config

- Use `atm config show` to inspect the active config file and current settings.
- Use `atm config init` to create a default config file.
- Useful config fields include `local.db_path`, `workspace.default`, and `workspace.routes`.
- `workspace.routes` maps paths to workspaces so commands run from those directories pick the right workspace.

## Discovery commands

```sh
atm list
atm list --project app
atm list --status todo
atm list --priority high
atm list --label bug
atm list --all
atm show APP-7KQ9
atm show APP-7KQ9 --full
atm projects
atm projects --search app
atm labels
atm labels --search bug
atm workspace list
atm config show
atm doctor
```

Use `show --full` before making decisions that depend on description, labels, notes, deletion state, or conflicts.

## Mutating commands

```sh
atm label create bug
atm project create app --path /path/to/repo
atm project path add app /path/to/repo
atm project path remove app /path/to/repo
atm add "fix conflict display" --project app --priority high --label bug
atm add "write docs" --project app --description-file notes.md
printf '## Context\nMarkdown works here\n' | atm add "write docs" --project app --description-stdin
atm update APP-7KQ9 --status active
atm update APP-7KQ9 --title "clearer title" --priority medium
atm update APP-7KQ9 --project app --label docs --remove-label bug
atm update APP-7KQ9 --description-file description.md
atm note APP-7KQ9 "handoff note"
printf 'handoff note\n' | atm note APP-7KQ9 --stdin
atm delete APP-7KQ9
atm restore APP-7KQ9
atm workspace create client-work
atm workspace rename client-work "Client Work"
```

After `atm add`, capture and report the printed ref so future agents can use it. Run `atm --help` or `atm <command> --help` for additional command details.

## Agent workflow

1. Confirm the active workspace with `atm doctor` when context is unclear.
2. Find work with `atm list` using focused filters.
3. Inspect the target with `atm show <ref> --full` before changing it.
4. Mark started work with `atm update <ref> --status active` when taking ownership.
5. Do the work outside `atm`.
6. Add durable context with `atm note <ref> ...` when the handoff needs more than the final status.
7. Mark finished work with `atm update <ref> --status done` only after the work is actually complete.
8. Commit repository changes before marking a development task done when the task required code changes.

## Conflicts

```sh
atm conflict list
atm conflict list --project app
atm conflict list --field title
atm conflict show APP-7KQ9
atm conflict show APP-7KQ9 --field description
atm conflict resolve APP-7KQ9 description --use <variant-token>
atm conflict resolve APP-7KQ9 description --value "final value"
atm conflict resolve APP-7KQ9 description --value-file value.md
```

- Inspect conflicts before resolving them.
- Use `conflict show` to see stable variant tokens.
- Resolve only after selecting the intended value.
- Do not bulk resolve conflicts or default to newest, local, or remote.

## Constraints for agents

- Use the task refs printed by `atm` output for task-specific commands.
- Use `--description-file`, `--description-stdin`, `note --file`, or `note --stdin` for long Markdown instead of shell-escaping large text.
- Avoid writing secrets into titles, descriptions, labels, projects, notes, or logs.
