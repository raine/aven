# Aven CLI Primer

`aven` is a local-first task manager. Use it to find and inspect work, maintain
task state, create follow-up tasks, and leave durable handoff context.

Do not create tasks unless the user asks or active agent instructions enable
automatic task creation.

## Basics

- Run commands from the repository whose tasks you want. Aven infers the
  workspace and project from the current directory.
- Use `aven doctor` when workspace, database, or project routing is unclear.
- Add `--workspace <name-or-key>` when a command must target another workspace.
- Let Aven infer the project unless the user specifies one or inference is
  incorrect.
- Run `aven <command> --help` for flags or operations not covered here.
- Status values are `inbox`, `backlog`, `todo`, `active`, `done`, and
  `canceled`.
- Priority values are `none`, `low`, `medium`, `high`, and `urgent`.
- Prefer human-readable output. Use `--json` for scripts and structured
  integrations.

## Task refs

- Use the refs printed by Aven, preferably qualified refs such as `APP-7KQ9`.
- The suffix identifies the task. The project prefix is display context, so it
  can change when the task changes projects.
- Bare suffix refs work when unambiguous. If Aven reports `ambiguous-ref`, retry
  with a longer suffix.
- Task refs identify the user's local tasks. Keep them out of commit messages,
  PR descriptions, and external systems.

## Find and inspect work

```sh
aven list --ready
aven list --open --project app --label bug
aven list --blocked
aven list --upcoming
aven list --overdue
aven list --deleted
aven search --project app "auth bug"
aven context APP-7KQ9
aven show APP-7KQ9 --full
```

- Use `list --ready` when choosing work. It excludes blocked tasks and epics.
- Use `context <ref>` for an agent-focused snapshot before acting. Use
  `show <ref> --full` when decisions depend on every task detail, including
  metadata.
- Search includes done and canceled tasks. Add `--all` to search deleted tasks.
- List commands accept filters such as `--status`, `--priority`, `--label`,
  `--metadata`, and `--project`. Use `--limit` to bound large results.

## Update tasks and leave handoff context

```sh
aven edit APP-7KQ9 --status active
aven edit APP-7KQ9 --title "Clearer title" --priority medium
aven note APP-7KQ9 --stdin <<'EOF'
Implemented the parser and documented the remaining edge case.
EOF
aven edit APP-7KQ9 --status done
```

- Mark work `active` when starting it and `done` only when the work is complete.
- Notes are append-style durable context for decisions, blockers, partial
  progress, and review findings. Keep scratch work elsewhere.
- Inspect dependency and conflict context before changing status or task order.
- Run `aven bulk-update ... --dry-run` before a broad mutation.
- `aven delete <ref>` soft-deletes a task. Recover it with
  `aven restore <ref>`.

## Create tasks

```sh
aven add "Fix conflict display" --status todo --priority high --label bug
aven add "Review migration plan" --metadata review-state=pending
aven add "Document retry behavior" --description-stdin <<'EOF'
Explain when retries are safe and add examples for each failure mode.

Acceptance criteria:
- The guide distinguishes transient and permanent failures.
- Every retry example is runnable.
EOF
```

- Plain tasks default to `inbox`. Use `--status` when the task is already
  triaged.
- Include enough context for a follow-up task to stand alone. Capture rationale,
  scope, acceptance criteria, and related tasks when established; do not invent
  missing details.
- Use `--description-stdin`, `--description-file`, `note --stdin`, or
  `note --file` for long Markdown instead of shell-escaping it.
- Capture and report the ref printed by `aven add`.
- Keep secrets out of titles, descriptions, labels, projects, metadata, notes,
  and logs.

## Relationships and scheduling

```sh
aven dep add APP-7KQ9 APP-7KQ0
aven add "Improve onboarding" --epic
aven epic add APP-7KQ9 APP-7KQ0
aven edit APP-7KQ9 --available-at tomorrow
aven edit APP-7KQ9 --due "next monday"
```

- `dep add <blocked> <blocker>` expresses execution order.
- `epic add <child> <epic>` groups related work. Epic membership does not block
  the child, so do not use epics and dependencies interchangeably.
- `available_at` defers attention until a date or time. `due_on` is a deadline
  and does not hide the task. Use `list --upcoming` and `list --overdue` to
  inspect them.
- Set metadata with `--metadata key=value`, remove it with
  `--remove-metadata key`, and filter with `--metadata key=value` or
  `--has-metadata key`.

## Recurring tasks

```sh
aven add "Daily journal" --repeat daily --repeat-at 09:00 \
  --time-zone Europe/Stockholm
aven recur show RCR-7KP2
aven recur edit RCR-7KP2 --title "Daily work journal"
aven recur pause RCR-7KP2
aven recur resume RCR-7KP2
aven recur stop RCR-7KP2
```

- Use an `RCR-...` series ref for recurrence commands. Use the ordinary task ref
  to complete the projected occurrence with `edit <ref> --status done`.
- Series edits affect future occurrences. Use `aven add --help` for recurrence
  rule and scheduling options.

## Attachments

```sh
aven attachment add APP-7KQ9 ./diagram.png --alt "Architecture diagram"
aven attachment list APP-7KQ9
aven attachment get 7KQ9A1X4MV2P8D6R --output diagram.png
aven attachment delete 7KQ9A1X4MV2P8D6R
```

Attachment commands keep image bytes outside descriptions and other text
output. Use the exact attachment ID printed by `attachment add` or
`attachment list`.

## Safe text edits and conflicts

For an existing long description, use the hash printed by `text get` to avoid
replacing a value that changed while you were editing it:

```sh
aven text get APP-7KQ9 description --output description.md
aven text diff APP-7KQ9 description --file description.md
aven text set APP-7KQ9 description --file description.md \
  --if-sha256 HASH_FROM_TEXT_GET
```

Inspect sync conflicts before resolving them:

```sh
aven conflict list
aven conflict show APP-7KQ9
aven conflict diff APP-7KQ9 description
aven conflict resolve APP-7KQ9 description --use TOKEN_FROM_CONFLICT_SHOW
```

Select the intended variant or provide an explicit value. Do not resolve
conflicts in bulk or assume the newest, local, or remote value is correct.
