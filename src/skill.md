# Aven CLI Primer

## Core model

- Binary: `aven`, a local-first task manager backed by SQLite.
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
- If `aven` reports `ambiguous-ref`, retry with a longer suffix.
- The project prefix is display context, not identity. If a task moves projects, the suffix remains stable and the prefix can change.

## Workspace and project selection

- Use `aven doctor` when the active workspace is unclear.
- Add `--workspace <name-or-key>` when a command must target a specific workspace.
- Project path routes can select the active workspace from the current directory.
- `project.overrides` maps paths to project names for inferred `aven add` tasks and `aven tui --project` or `aven tui -p` startup. Plain `aven tui` starts in the all-projects queue.

## Discovery commands

```sh
aven list
aven list --project app
aven list --project app --all
aven list --status todo
aven list --all
aven show APP-7KQ9 --full
aven projects
aven project path list
aven labels
aven workspace list
aven config show
aven doctor
```

Use `prime` to print this primer plus open issues for the inferred current project. Open issues are tasks that are not done, not canceled, and not deleted. Use `list --all` with any normal list filters to find deleted tasks, for example `aven list --project app --all`. Use `show --full` before making decisions that depend on description, labels, notes, deletion state, or conflicts.

## Mutating commands

```sh
aven label create bug
aven project create app --path /path/to/repo
aven project path add app /path/to/repo
aven add "fix inferred project task"
aven add "fix conflict display" --project app --priority high --label bug
aven add "write docs" --project app --description-file notes.md
aven add "in slack-agent, we need to fix dispatch" --natural
aven tmux add-task-popup --print-binding
aven update APP-7KQ9 --status active
aven update APP-7KQ9 --title "clearer title" --priority medium
aven update APP-7KQ9 --project app --label docs --remove-label bug
aven bulk-update --filter-label bug --remove-label bug --dry-run
aven note APP-7KQ9 "handoff note"
printf 'handoff note\n' | aven note APP-7KQ9 --stdin
aven delete APP-7KQ9
aven restore APP-7KQ9
aven workspace create client-work
```

After `aven add`, capture and report the printed ref so future agents can use it. `aven add --natural` sends the title argument as raw intake text to `agent.task_intake.command`, which must return JSON for a task draft. The command is configured with argv pieces, not a shell string. Use `{prompt}` in `agent.task_intake.args` for commands such as `claude -p`; otherwise the prompt is written to stdin. Use `aven tmux add-task-popup --print-binding` to print a tmux binding for opening `aven tui --add-task` in a popup. For `bulk-update`, `--filter-label` selects tasks and `--label` adds a label. Prefer `--dry-run` before broad bulk mutations. Run `aven --help` or `aven <command> --help` for additional command details.

## Agent workflow

1. Confirm the active workspace with `aven doctor` when context is unclear.
2. Find work with `aven list` using focused filters.
3. Inspect the target with `aven show <ref> --full` before changing it.
4. Mark started work with `aven update <ref> --status active` when taking ownership.
5. Do the work outside `aven`.
6. Add durable context with `aven note <ref> ...` when the handoff needs more than the final status.
7. Mark finished work with `aven update <ref> --status done` only after the work is actually complete.
8. Commit repository changes before marking a development task done when the task required code changes.

## Conflicts

```sh
aven conflict list
aven conflict show APP-7KQ9
aven conflict resolve APP-7KQ9 description --use <variant-token>
aven conflict resolve APP-7KQ9 description --value-file value.md
```

- Inspect conflicts before resolving them.
- Use `conflict show` to see stable variant tokens.
- Resolve only after selecting the intended value.
- Do not bulk resolve conflicts or default to newest, local, or remote.

## Constraints for agents

- Use the task refs printed by `aven` output for task-specific commands.
- Use `--description-file`, `--description-stdin`, `note --file`, or `note --stdin` for long Markdown instead of shell-escaping large text.
- Avoid writing secrets into titles, descriptions, labels, projects, notes, or logs.
