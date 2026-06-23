# Aven CLI Primer

`aven` is a local-first task manager backed by SQLite. Use it to find work,
inspect tasks, update status, and leave durable handoff context.

## Start here

- Run `aven prime` in the repo. It prints this primer plus open issues for the
  inferred project.
- Use `aven prime --project <project>` when project inference is wrong or
  unavailable.
- Use `aven doctor` when the active workspace, database, or project routing is
  unclear.
- Run `aven <command> --help` when you need flags not shown here.

## Task refs and values

- Use refs printed by command output, preferably qualified refs like `APP-7KQ9`.
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
aven show APP-7KQ9 --full
aven add "fix conflict display" --project app --priority high --label bug
aven update APP-7KQ9 --status active
aven update APP-7KQ9 --title "clearer title" --priority medium
aven note APP-7KQ9 "durable handoff context"
aven delete APP-7KQ9
aven restore APP-7KQ9
```

- Use `show --full` before decisions that depend on description, labels, notes,
  deletion state, or conflicts.
- After `aven add`, capture and report the printed ref so future agents can use
  it.
- Use `list --all` with normal filters to find deleted tasks.
- Use `bulk-update --dry-run` before broad mutations.
- `aven add --natural` sends raw intake text to `agent.task_intake.command`,
  which must return JSON for a task draft.
- The task intake command is configured as argv pieces, not a shell string. Use
  `{prompt}` in `agent.task_intake.args` for commands such as `claude -p`;
  otherwise the prompt is written to stdin.
- Configure `agent.task_intake.system_prompt` as the full task intake prompt
  template. Supported placeholders are `{priorities}`, `{inferred_project}`,
  `{projects}`, `{labels}`, and `{input}`. Omit a placeholder to leave that
  context out of the prompt.
- Use `aven tmux add-task-popup --print-binding` for a tmux binding that opens
  `aven tui --add-task-only` in a popup.
- Use `aven tui --add-task` for the full TUI with the add-task dialog active.
- Add `--natural` to either add-task entry point, or type natural-language text
  in the add-task title field and press `Ctrl+N` to parse and prefill the form.

## Long input and secrets

- Use `--description-file`, `--description-stdin`, `note --file`, or
  `note --stdin` for long Markdown instead of shell-escaping large text.
- Notes are append-style and better for durable handoff context than scratch
  work.
- Avoid writing secrets into titles, descriptions, labels, projects, notes, or
  logs.

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
