---
title: Command reference
description: Complete syntax, arguments, options, behavior, and examples for the aven CLI.
---

This page documents every user-facing `aven` command. The command line accepts global options before or after a subcommand.

## Global syntax

```sh
aven [--db <path>] [--workspace <name-or-key>] <command>
aven help [<command>...]
aven --version
```

| Option | Description |
| --- | --- |
| `--db <path>` | Use a specific SQLite database. This takes precedence over `AVEN_DB`, `local.db_path`, and the default state directory. |
| `--workspace <name-or-key>` | Use a specific workspace instead of directory routing or the configured default. |
| `-h`, `--help` | Print help for the selected command. |
| `-V`, `--version` | Print the installed aven version. |

`aven help <command>` and `aven <command> --help` show the same command-specific help. Nested help accepts the full command path, such as `aven help project path add`.

## Common values and behavior

### Task references

Commands accept the qualified references printed by aven, such as `APP-7KQ9`, or an unambiguous bare suffix such as `7KQ9`. Typed suffixes must contain at least three characters. Aven lengthens displayed suffixes when needed to keep them unambiguous. `O` is treated as `0`, and `I` or `L` is treated as `1` when resolving a reference.

### Statuses and priorities

Valid statuses are `inbox`, `backlog`, `todo`, `active`, `done`, and `canceled`.

Valid priorities are `none`, `low`, `medium`, `high`, and `urgent`.

### Text input

Commands that accept long text provide mutually exclusive inline, file, and standard-input forms. Pass only one source for a field. For example:

```sh
aven add "Document recovery" --description "Short description"
aven add "Document recovery" --description-file ./description.md
aven add "Document recovery" --description-stdin <<'EOF'
Explain the recovery workflow.
Include a verification step.
EOF
```

### Structured output

The `--json` option is available on `context`, `show`, `list`, `search`, `recur list`, `recur show`, `recur history`, `attachment add`, `attachment list`, `attachment get`, `attachment delete`, `attachment prune`, `dep list`, `epic list`, `prime`, `label list`, `project list`, `conflict list`, `conflict show`, and `doctor`. JSON task objects expose temporal fields through `available_at` and `due_on`, and epic membership separately from dependency ordering through `is_epic`, `epic_parent`, and `epic_children`. `context --json` and `show --full --json` also expose attachment metadata without bytes. An empty `available_at` means immediate availability. An empty `due_on` means no deadline.

## Temporal input

### Availability input

Availability controls when a task becomes eligible for attention and enters normal task lists and queue groups. It is not a deadline or due date. An empty `available_at` value means the task is immediately available. `aven add --available-at`, `aven edit --available-at`, natural task intake, and the TUI task composer use the same expressions:

| Expression | Local-calendar meaning |
| --- | --- |
| `today` | Today at local midnight. |
| `tomorrow` | Tomorrow at local midnight. |
| `Nd`, `Nw` | Local midnight after `N` calendar days or weeks, such as `2d` or `3w`. |
| `in N days` | Local midnight after `N` calendar days. Singular `day` is accepted. |
| `in N weeks` | Local midnight after `N * 7` calendar days. Singular `week` is accepted. |
| `in N months` | The same local day after `N` calendar months. End-of-month dates clamp to the last valid day. Singular `month` is accepted. |
| `next week` | The next Monday strictly after today at local midnight. |
| `next monday` | The named weekday strictly after today at local midnight. Full names and common abbreviations such as `mon`, `tues`, and `thurs` are accepted. If today is that weekday, this means seven days later. |
| `<date expression> at <time>` | The expression's local date at `HH:MM`, `9am`, `9:30pm`, `noon`, or `midnight`. |
| `YYYY-MM-DD` | Local midnight on that date. |
| `YYYY-MM-DDTHH:MM:SSZ` | The exact UTC timestamp. The same form without `Z` is also interpreted as UTC. |
| Unix timestamp | The exact whole number of seconds since the Unix epoch. |
| `now` | Immediate availability. On `aven edit`, `--clear-available-at` is clearer. |

Relative expressions add calendar units to the current local date rather than fixed durations. Local expressions use the machine's local timezone and convert to a canonical UTC timestamp for storage. This preserves the requested local wall-clock time across daylight-saving offsets. A local time that is skipped or repeated by a timezone transition is rejected, so use another local time or an explicit UTC timestamp.

Bare weekdays such as `monday` and bare times such as `9am` are ambiguous and rejected with a suggested explicit form. Informal periods such as `morning`, recurrence, year arithmetic, and notifications are outside the accepted grammar.

```sh
aven add "Prepare demo" --available-at "in 2 weeks"
aven add "Call supplier" --available-at "next monday at 9am"
aven edit APP-7KQ9 --available-at "tomorrow at 14:30"
```

### Due date input

A due date describes when completion is expected. It does not control visibility, change status, clear availability, or create a reminder or notification. A task can be available before its deadline, deferred beyond its deadline, or have either field on its own.

`aven add --due`, `aven edit --due`, natural task intake, and the TUI use the date expressions from [Availability input](#availability-input), including compact offsets, calendar months, `next week`, named weekdays, and ISO dates. Due dates are local-calendar values stored as `YYYY-MM-DD`. They reject times, UTC timestamps, and Unix timestamps because a deadline is date-only.

Use `none`, `clear`, or `aven edit --clear-due` to remove a deadline. A due date equal to today is due today. It becomes overdue when the local date advances past it.

```sh
aven add "Submit report" --due "next fri"
aven edit APP-7KQ9 --due "in 1 month"
aven edit APP-7KQ9 --clear-due
aven list --overdue
```

### Recurrence input

`aven add --repeat` accepts these exact forms:

| Value | Schedule |
| --- | --- |
| `daily` | Every day. |
| `weekdays` | Monday through Friday. |
| `weekly` | Every week on the start date's weekday. |
| `fortnightly` | Every two weeks on the start date's weekday. |
| `monthly` | The start date's day number every month. |
| `weekly on mon,wed,fri` | The selected weekdays every week. |
| `every N weeks` | Every N weeks on the start date's weekday. |
| `every N weeks on mon,thu` | The selected weekdays every N weeks. |

Explicit weekdays use the canonical abbreviations `mon`, `tue`, `wed`, `thu`,
`fri`, `sat`, and `sun`, listed in Monday-to-Sunday order. `N` must be a
positive whole number. Weeks begin on Monday.

Monthly schedules use the final day of shorter months. A schedule starting on
January 31 runs on February 28 or 29, then March 31.

The TUI schedule field also accepts friendly forms such as `every day`, `every
Friday`, `Fridays`, `every month`, and `every 3 weeks on Monday and Thursday`.
See [Recurring tasks](/recurring-tasks/) for creation workflows and schedule
behavior.

## Task commands

### `aven add`

Create a task in the active workspace.

```sh
aven add <title> [options]
```

| Argument or option | Description |
| --- | --- |
| `<title>` | Task title. With `--natural`, this is the natural-language task request. |
| `--project <project>` | Create the task in this project. Without it, aven infers a project from the current directory and creates that project when needed. |
| `--description <text>` | Set the description inline. |
| `--description-file <path>` | Read the description from a UTF-8 file. |
| `--description-stdin` | Read the description from standard input. |
| `--priority <priority>` | Set the priority. Defaults to `none`. |
| `--available-at <when>` | Defer the task until a calendar expression or timestamp. See [Availability input](#availability-input). |
| `--due <when>` | Set a date-only deadline. See [Due date input](#due-date-input). |
| `--repeat <rule>` | Create a recurring task using a value from [Recurrence input](#recurrence-input). |
| `--repeat-at <HH:MM>` | Make each dated task available at this local time in the recurring task's time zone. |
| `--repeat-due <same-day\|none>` | Make each dated task due on its scheduled date or leave it without a due date. Defaults to `same-day`. |
| `--time-zone <IANA-zone>` | Interpret scheduled dates and local times in this IANA time zone. Defaults to the device zone. |
| `--repeat-start-on <YYYY-MM-DD>` | Set the first eligible scheduled date. Defaults to the creation date in the recurring task's time zone. |
| `--label <label>` | Add a label. Repeat for multiple labels. Labels must already exist. |
| `--epic` | Create an epic container. |
| `--natural` | Parse the title as natural-language task intake using the configured agent command. |

A plain task starts with status `inbox`. A recurring task starts with status
`todo` and prints both its stable `RCR-` reference and first task reference. See
[Recurrence input](#recurrence-input) for the accepted `--repeat` values.
`--available-at` and `--due` cannot be combined with `--repeat`; use
`--repeat-at` and `--repeat-due` instead.

`--natural` cannot be combined with a description source, `--project`, a
non-default priority, `--label`, scheduling flags, or `--due`. Natural intake
can infer a title, description, project, status, priority, labels, availability,
due date, and epic state from the request.

The command prints the created task's qualified reference and bare suffix.

```sh
aven add "Fix conflict display" --project aven --priority high --label bug
aven add "Review launch notes" --available-at "next monday at 9am"
aven add "Submit expense report" --due "next fri"
aven add "Add release automation" --epic
aven add "Daily journal" --repeat daily --repeat-at 09:00 --time-zone Europe/Stockholm
aven add "Review invoices" --repeat monthly --repeat-start-on 2026-01-31
aven add "Plan next sprint" --repeat fortnightly --repeat-start-on 2026-07-27
aven add --natural "high priority docs task for the aven project"
```

### `aven list`

List tasks in the active workspace. Results are ordered by most recent update first. By default, deleted tasks are excluded.

```sh
aven list [options]
```

| Option | Description |
| --- | --- |
| `--project <project>` | Match one project. |
| `--status <status>` | Match one status. |
| `--priority <priority>` | Match one priority. |
| `--label <label>` | Match one label. |
| `--all` | Include deleted tasks alongside live tasks. |
| `--deleted` | Show deleted tasks only. Normal filters still apply. |
| `--ready` | Show open, non-epic tasks with no unresolved blocker. |
| `--blocked` | Show open tasks with at least one unresolved blocker. |
| `--epics` | Show epic containers only. |
| `--upcoming` | Show open, live tasks with a future availability time, ordered by availability time from earliest to latest. |
| `--overdue` | Show open, live tasks with a due date before today, ordered by due date from oldest to newest. |
| `--expand-recurring` | Show each dated recurring task as a separate row instead of grouping past tasks by recurring task. |
| `--limit <number>` | Return at most this many tasks after sorting and filtering. |
| `--json` | Print a JSON array. |

Normal lists hide open tasks whose availability time is in the future. They include tasks with empty or elapsed availability. Explicit `--status done` and `--status canceled` lists include matching tasks regardless of availability, and `--deleted` includes matching deleted tasks regardless of availability.

`--upcoming` finds only live tasks with an open status and a future availability time. It can be combined with `--project`, `--status`, `--priority`, `--label`, `--all`, `--overdue`, and `--limit`. `--all` does not add deleted tasks to Upcoming. `--upcoming` cannot be combined with `--ready`, `--blocked`, `--epics`, or `--deleted`.

`--overdue` finds open, live tasks whose `due_on` date is earlier than the current local date. Due today is not overdue. Normal availability filtering still applies, so deferred overdue tasks remain hidden. Combine `--upcoming --overdue` to inspect deferred tasks whose deadlines passed.

`--ready` and `--blocked` are mutually exclusive. Neither can be combined with `--all` or `--deleted`. `--ready` and `--epics` are also mutually exclusive.

```sh
aven list --status active
aven list --project aven --label bug --limit 20
aven list --ready
aven list --upcoming
aven list --upcoming --project aven --limit 10
aven list --overdue
aven list --upcoming --overdue
aven list --deleted --json
```

### `aven search`

Search task references, titles, descriptions, projects, labels, notes, statuses, priorities, and current attachment filenames and alternative text across the active workspace. Done and canceled tasks participate in normal search. Deleted tasks require `--all`.

```sh
aven search <query>... [--limit <number>] [--all] [--json]
```

| Argument or option | Description |
| --- | --- |
| `<query>...` | One or more search terms. Multiple shell arguments are joined with spaces. |
| `--limit <number>` | Maximum result count. Defaults to `50`. |
| `--all` | Include deleted tasks. |
| `--expand-recurring` | Return each matching dated task instead of grouping past matches by recurring task. |
| `--json` | Print ranked results as JSON, including score, matched field, and optional snippet. |

Text output identifies the matched field and score and prints a snippet when available. Ref-shaped input can resolve a deleted task when `--all` is present.

```sh
aven search "auth bug"
aven search APP-7KQ9
aven search recovery --all --json
```

### `aven context`

Print a complete context snapshot for one task.

```sh
aven context <task-ref> [--json]
```

The snapshot includes stable identity, display reference, task fields, description, project and workspace metadata, labels, notes, dependencies, dependents, epic membership, recurring-task metadata, deletion state, and unresolved conflicts. It also derives whether the task is blocked, has conflicts, or blocks open work.

```sh
aven context APP-7KQ9
aven context APP-7KQ9 --json
```

### `aven show`

Show one task.

```sh
aven show <task-ref> [--full] [--json]
```

| Option | Description |
| --- | --- |
| `--full` | Include the description, project prefix, dependency summary, notes, and unresolved conflicts. |
| `--json` | Print JSON. With `--full`, emit the expanded task object. |

The compact form prints the task's main fields and relationships. Use `context` when workspace and project metadata or derived state flags are also needed.

```sh
aven show APP-7KQ9
aven show APP-7KQ9 --full
aven show APP-7KQ9 --full --json
```

### `aven recur`

Inspect and manage recurring tasks. See [Recurring tasks](/recurring-tasks/) for
creation workflows, schedule behavior, and lifecycle guidance. Every command
that takes a reference accepts the recurring task's `RCR-7KP2` reference or any
linked task reference.

#### `recur list`

```sh
aven recur list [--json]
```

Print one row per recurring task with its active, paused, or stopped state,
schedule, time zone, next task reference, and history counts.

#### `recur show`

```sh
aven recur show <recurring-or-task-ref> [--json]
```

Show the schedule, settings used for future tasks, next task, history summary,
and unresolved state conflicts.

#### `recur history`

```sh
aven recur history <recurring-or-task-ref> \
  [--offset <number>] [--limit <number>] [--json]
```

History includes completed, skipped, and missed dates plus pause periods. A
missed date may include a task reference when Aven created a task for that date.
`--offset` defaults to `0`, and `--limit` defaults to `100`.

The JSON output from `recur list`, `recur show`, and `recur history` uses
`version: 1` and a command-specific `kind` value.

#### `recur edit`

```sh
aven recur edit <recurring-or-task-ref> [options]
```

| Option | Description |
| --- | --- |
| `--title <title>` | Change the title used by future tasks. |
| `--description <text>` | Change the future description inline. |
| `--description-file <path>` | Read the future description from a UTF-8 file. |
| `--description-stdin` | Read the future description from standard input. |
| `--project <project>` | Change the project used by future tasks. |
| `--status <status>` | Change the starting status used by future tasks. |
| `--priority <priority>` | Change the priority used by future tasks. |
| `--label <label>` | Replace future labels. Repeat for multiple labels. |
| `--repeat-at <HH:MM\|none>` | Change or clear the future availability time. |
| `--repeat-due <same-day\|none>` | Change the future due setting. |

These options affect tasks created later. They do not rewrite the current task
or history. The repetition pattern, start date, and time zone cannot be edited.

#### `recur skip`

```sh
aven recur skip <recurring-or-task-ref>
```

Mark the current task canceled, record its scheduled date as skipped, and
create the next task. Setting the current task's ordinary status to `canceled`
has the same result.

#### `recur pause` and `recur resume`

```sh
aven recur pause <recurring-or-task-ref>
aven recur resume <recurring-or-task-ref>
```

Pause hides the current task from normal active views and suppresses scheduled
dates until resume. Resume continues without creating tasks for the paused
period.

#### `recur stop`

```sh
aven recur stop <recurring-or-task-ref> [--skip-current]
```

Stop ends future scheduling and leaves the current task available as the last
one. `--skip-current` also marks that task canceled and records its date as
skipped.

Done lists and search results group past tasks by recurring task. Pass
`--expand-recurring` to `aven list` or `aven search` for one row per dated task.
Task JSON includes nullable `recurrence` and `recurrence_group` objects.

### `aven attachment`

Manage task image attachments without changing task descriptions.

```sh
aven attachment add <task-ref> <path> [--alt <text>] [--filename <name>] [--media-type <type>] [--optimize | --no-optimize] [--json]
aven attachment list <task-ref> [--all] [--json]
aven attachment get <attachment-id> [--output <path>] [--all] [--json]
aven attachment delete <attachment-id> [--json]
aven attachment prune [--dry-run | --apply] [--json]
```

Attachment metadata determines identity, display order, and search matches. Attachment IDs are exact, workspace-scoped 16-character uppercase Crockford Base32 identifiers. Task-ref suffix and normalization rules do not apply. Live attachments appear in `(created_at, attachment_id)` order. Attachment `list` and `get` require `--all` to include tombstoned metadata. Human task sections, `aven context --json`, and `aven show --full --json` expose live attachments only. Compact `aven show --json` has no attachment array.

#### Add an image

Create the task first, then attach an image from a filesystem path:

```sh
aven attachment add APP-7KQ9 ./diagram.png --alt "service layout"
aven attachment add APP-7KQ9 ./capture --filename error.webp --media-type image/webp
```

The filename defaults to the file's basename. Use `--filename` to choose another display name and `--alt` to add searchable alternative text. `--media-type` checks that the file has the expected format; it does not convert the image. The regular `aven add` command has no attachment option.

#### List and retrieve images

`list` shows a task's current attachments in the order they were added. Pass `--all` to include deleted attachments. Human-readable output is compact; use `--json` for filenames, alternative text, dimensions, and local file availability.

```sh
aven attachment list APP-7KQ9
aven attachment list APP-7KQ9 --json
```

Copy the full attachment ID from this output when using `get` or `delete`. Attachment IDs are exact, workspace-scoped 16-character uppercase Crockford Base32 values and cannot be abbreviated like task refs.

`get` prints attachment information by default. Add `--output` to save the image to a file. Aven refuses to overwrite an existing path.

```sh
aven attachment get 7KQ9A1X4MV2P8D6R
aven attachment get 7KQ9A1X4MV2P8D6R --output ./diagram.png
```

Saving the file requires the image to be available on this device. In JSON output, `has_blob: true` means it is available. If it is not, run sync or restore a [backup that includes images](/sync/#back-up-and-move-data).

#### Delete an attachment

`delete` removes the attachment from its task. Repeating the command is safe. Aven has no attachment restore command.

```sh
aven attachment delete 7KQ9A1X4MV2P8D6R
```

The image file can remain during the configured grace period. While it remains available, retrieve it and add it again as a new attachment:

```sh
aven attachment list APP-7KQ9 --all
aven attachment get 7KQ9A1X4MV2P8D6R --all --output ./recovered.png
aven attachment add APP-7KQ9 ./recovered.png
```

Pruning can remove an unused image after the grace period. Normal sync downloads images for current attachments and does not restore a deleted attachment, so use a backup when the local file is unavailable.

#### Supported images and optimization

Aven checks the actual image rather than trusting its extension. It accepts nonempty PNG, JPEG, GIF, and WebP files with these limits:

| Item | Limit |
| --- | --- |
| Image file | 25 MiB |
| Width or height | 16,384 pixels |
| Pixels in one frame | 40,000,000 |
| Animation frames | 100 |
| Pixels across an animation | 100,000,000 |
| Filename | 1-255 UTF-8 bytes, with no control characters or path separators |
| Alternative text | 500 UTF-8 bytes, with no control characters |

Aven checks the complete image, including every animation frame, before attaching it. `--optimize` requests lossless PNG optimization, while `--no-optimize` preserves the file. Without either flag, [`local.image_optimization`](/configuration/#png-optimization) controls CLI attachments and defaults to `off`. Optimization applies only to PNG files. Aven preserves the original when optimization fails or does not reduce the file size, and validates any optimized file before storage.

#### JSON output and search

JSON output from `add`, `list`, `get`, and `delete` contains `attachment_id`, `task_id`, `sha256`, `byte_size`, `media_type`, `filename`, `alt_text`, `width`, `height`, `created_at`, `deleted`, `deleted_at`, and `has_blob`. Add output also contains `optimized`.

`context --json` and `show --full --json` include current attachment information except `sha256`. Deleted attachments are excluded; use `attachment list --all` or `attachment get --all` to inspect them. Compact `show --json` has no attachment array. Search matches current attachment filenames and alternative text, not image contents or hashes.

#### Clean up unused images

`attachment prune` reports image files eligible for cleanup. It performs a dry run by default. Pass `--apply` to delete them. JSON output contains `mode`, `eligible_count`, `eligible_bytes`, `pruned_count`, and `pruned_bytes`.

Cleanup honors the configured grace period and processes at most [`local.attachment_lifecycle.maintenance_limit`](/configuration/#retention-and-storage-limits) files per run, so large cleanups may require repeated runs. Images used by tasks or attachment operations in progress are protected. Identical images share storage and count once toward attachment quotas. Cached previews are disposable and may be cleaned during either a dry run or an applied cleanup.

### `aven edit`

Update fields on one task.

```sh
aven edit <task-ref> [options]
```

| Option | Description |
| --- | --- |
| `--title <title>` | Replace the title. |
| `--description <text>` | Replace the description inline. |
| `--description-file <path>` | Replace the description from a UTF-8 file. |
| `--description-stdin` | Replace the description from standard input. |
| `--project <project>` | Move the task to an existing project. |
| `--status <status>` | Set the status. |
| `--priority <priority>` | Set the priority. |
| `--available-at <when>` | Set or reschedule availability with a calendar expression or timestamp. Passing `now` makes the task immediately available. See [Availability input](#availability-input). |
| `--clear-available-at` | Clear availability so the task is immediately available. |
| `--due <when>` | Set or reschedule the date-only deadline. Passing `none` or `clear` removes it. See [Due date input](#due-date-input). |
| `--clear-due` | Remove the due date. |
| `--epic <on-or-off>` | Set epic state. Accepted true values are `on`, `true`, and `1`; accepted false values are `off`, `false`, and `0`. |
| `--label <label>` | Add an existing label. Repeat as needed. |
| `--remove-label <label>` | Remove a label. Repeat as needed. |

Description sources are mutually exclusive. `--available-at` and `--clear-available-at` are mutually exclusive. `--due` and `--clear-due` are mutually exclusive. Aven reports whether the update changed the task.

A task with epic children cannot have epic state turned off. Epic containers cannot become children of another epic.

```sh
aven edit APP-7KQ9 --status active
aven edit APP-7KQ9 --title "Clarify conflict output" --priority medium
aven edit APP-7KQ9 --available-at "in 2 weeks at 14:30"
aven edit APP-7KQ9 --clear-available-at
aven edit APP-7KQ9 --due "in 1 month"
aven edit APP-7KQ9 --clear-due
aven edit APP-7KQ9 --label docs --remove-label bug
aven edit APP-7KQ0 --epic on
```

### `aven note`

Append a durable note to a task.

```sh
aven note <task-ref> [text] [--file <path> | --stdin]
```

Supply exactly one of `[text]`, `--file`, or `--stdin`. The command prints the generated note ID, which `note-delete` accepts.

```sh
aven note APP-7KQ9 "Waiting for API review"
aven note APP-7KQ9 --file ./handoff.md
printf '%s\n' "Reproduces on clean install" | aven note APP-7KQ9 --stdin
```

### `aven note-delete`

Delete a note from a task. Repeating the command for an already-deleted note succeeds and reports that no change was made.

```sh
aven note-delete <task-ref> <note-id>
```

```sh
aven note-delete APP-7KQ9 01JABCDEF123
```

### `aven dep`

Inspect and modify task dependencies. In `dep add <task> <blocker>`, the first task depends on the second task.

```sh
aven dep add <task-ref> <depends-on-ref>
aven dep remove <task-ref> <depends-on-ref>
aven dep list <task-ref> [--json]
```

Dependencies must stay within one workspace. A task cannot depend on itself, and aven rejects dependency cycles. Adding or removing the same relationship repeatedly is safe and reports whether anything changed. A dependency is unresolved while its blocker is open.

```sh
aven dep add APP-7KQ9 APP-7KQ0
aven dep list APP-7KQ9
aven dep remove APP-7KQ9 APP-7KQ0
```

### `aven epic`

Inspect and modify epic membership.

```sh
aven epic add <child-ref> <epic-ref>
aven epic remove <child-ref> <epic-ref>
aven epic list <epic-ref> [--json]
```

`epic add` marks the parent as an epic when needed. Child and epic must belong to the same workspace and project. An epic cannot be its own child, epic containers cannot be children, and each child can belong to only one epic. Epic membership does not create a dependency or make a child blocked.

`epic list` prints the epic and all children, including child status, priority, and title.

```sh
aven epic add APP-7KQ9 APP-7KQ0
aven epic list APP-7KQ0 --json
aven epic remove APP-7KQ9 APP-7KQ0
```

### `aven text`

Read and update long text with optimistic concurrency protection. The only supported text field is `description`.

```sh
aven text get <task-ref> description [--raw | --output <path>]
aven text diff <task-ref> description --file <path>
aven text set <task-ref> description (--file <path> | --stdin) --if-sha256 <hash>
```

#### `text get`

Prints the current SHA-256 hash and description. `--raw` writes only the description with no added newline. `--output` writes the description to a file and prints its hash. Do not combine `--raw` and `--output` if downstream output must remain machine-oriented, because `--output` takes precedence.

#### `text diff`

Reads the candidate UTF-8 file and prints a unified-style comparison against the stored description. It does not modify the task.

#### `text set`

Reads the replacement from exactly one of `--file` or `--stdin`. The write succeeds only when `--if-sha256` matches the hash of the stored description, preventing an unseen concurrent edit from being overwritten.

```sh
aven text get APP-7KQ9 description --output description.md
aven text diff APP-7KQ9 description --file description.md
aven text set APP-7KQ9 description --file description.md --if-sha256 <hash-from-get>
```

### `aven bulk-update`

Apply the same field changes to a selected set of tasks.

```sh
aven bulk-update <selector>... <mutation>... [--dry-run] [--include-deleted]
```

Selectors:

| Option | Description |
| --- | --- |
| `--project <project>` | Select tasks in one project. |
| `--status <status>` | Select tasks with one status. |
| `--priority <priority>` | Select tasks with one priority. |
| `--filter-label <label>` | Select tasks with one label. |
| `--all` | Explicitly select all live tasks when no narrower filter is appropriate. |
| `--include-deleted` | Include deleted tasks in the selected set. This is not itself a selector. |

Mutations:

| Option | Description |
| --- | --- |
| `--set-status <status>` | Set status. |
| `--set-priority <priority>` | Set priority. |
| `--set-project <project>` | Move tasks to an existing project. |
| `--label <label>` | Add an existing label. Repeat as needed. |
| `--remove-label <label>` | Remove a label. Repeat as needed. |
| `--dry-run` | Print the planned result without writing. |

At least one selector and one mutation are required. Selector options combine as an intersection. The same label cannot appear in both `--label` and `--remove-label`. Before any write, aven validates the entire selected set and rejects updates to fields with unresolved conflicts. Output includes one line per task and a matched, changed, unchanged, and dry-run summary.

```sh
aven bulk-update --project aven --status todo --set-priority high --dry-run
aven bulk-update --filter-label old --label migrated --remove-label old
aven bulk-update --all --set-status backlog
```

### `aven delete`

Soft-delete one task.

```sh
aven delete <task-ref>
```

Deleted tasks stay in the database and can be found with `list --deleted`, `list --all`, or `search --all`.

```sh
aven delete APP-7KQ9
```

### `aven restore`

Restore one soft-deleted task.

```sh
aven restore <task-ref>
```

```sh
aven restore APP-7KQ9
```

## Workspace commands

### `aven workspace`

Manage task universes in the database.

```sh
aven workspace list
aven workspace create <name>
aven workspace rename <workspace> <new-name>
```

`list` prints every workspace key and name. `create` derives a normalized key from the name. `rename` accepts a workspace name or key and updates its name and key.

```sh
aven workspace create "Client work"
aven workspace rename client-work "Consulting"
aven workspace list
```

### `aven project`

Manage projects in the active workspace.

```sh
aven project create <name> [--path <path>]
aven project delete <project>
aven project list [--search <text>] [--limit <number>] [--json]
aven project rename <project> <new-name> [--prefix <prefix>]
aven project path add <project> <path>
aven project path remove <project> <path>
aven project path list [project]
```

#### `project create`

Creates a project with a normalized key and a unique display prefix. `--path` also records a canonical directory mapping in the config file.

#### `project delete`

Deletes a project from normal use and removes its path mappings. Projects referenced by tasks remain stored so those tasks retain project identity.

#### `project list`

Lists project keys, prefixes, and names. `--search` performs project search, `--limit` truncates results, and `--json` returns objects with `key`, `name`, and `prefix`.

#### `project rename`

Updates the project's name, normalized key, and display prefix while preserving stable project identity. Without `--prefix`, aven generates a unique prefix from the new key. Existing managed config mappings are rewritten to the new project key.

#### `project path`

`add` and `remove` update managed path mappings in the config file. Paths are canonicalized. `list` prints all mappings or only mappings for the optional project.

```sh
aven project create "Aven docs" --path ~/code/aven/docs
aven project rename aven-docs "Documentation" --prefix DOC
aven project path add documentation ~/writing/aven
aven project path list documentation
aven project list --search doc --json
```

### `aven label`

Manage labels in the active workspace.

```sh
aven label create <name>
aven label delete <name>
aven label list [--search <text>] [--limit <number>] [--json]
```

Label names are normalized. Labels must exist before `add`, `edit`, or `bulk-update` assigns them. Deleting a label removes its task assignments. Repeating deletion is safe and reports whether anything changed.

```sh
aven label create bug
aven label list --search bu
aven label delete bug
```

## Sync commands

### `aven sync`

Push local changes and pull remote changes until both backlogs are drained.

```sh
aven sync [--server <url>]
```

The server URL resolves from `--server`, then `AVEN_SYNC_SERVER`, then `sync.server_url`. When configured, `sync.auth_token` is sent as bearer authentication. Aven pins the normalized server URL in database metadata and rejects accidental reuse of one database with a different server.

Output reports pushed and pulled task changes, uploaded and downloaded image counts and sizes, remaining work, the resulting server cursor, and completion state. Large attachment backlogs transfer in bounded rounds, and `complete=false` means sync still has work to do.

```sh
aven sync
aven sync --server http://127.0.0.1:3000
```

### `aven server`

Run an HTTP sync server backed by its own SQLite database.

```sh
aven server --data <path> [--bind <address>] [--unsafe-public-bind]
```

| Option | Description |
| --- | --- |
| `--data <path>` | Required server SQLite database path. |
| `--bind <ip:port>` | Listen address. Defaults to `127.0.0.1:0`, which chooses an available loopback port. |
| `--unsafe-public-bind` | Allow a public IP bind. Public binds also require `sync.auth_token`. |

Loopback binds can run without authentication. Private-network binds require `sync.auth_token`. Public binds require both an auth token and `--unsafe-public-bind`; aven prints a warning to use TLS or a reverse proxy. The server exposes `POST /sync`, accepts compressed requests, compresses responses when appropriate, and shuts down gracefully on an operating-system termination signal.

```sh
aven server --data ~/.local/share/aven/server.sqlite --bind 127.0.0.1:3000
aven server --data /srv/aven/server.sqlite --bind 192.168.1.10:3000
```

### `aven conflict`

Inspect and resolve unresolved sync conflicts.

```sh
aven conflict list [--project <project>] [--field <field>] [--limit <number>] [--json]
aven conflict show <task-ref> [--field <field>] [--json]
aven conflict diff <task-ref> <field>
aven conflict export <task-ref> <field> --dir <path>
aven conflict resolve <task-ref> <field> (--use <variant> | --value <text> | --value-file <path> | --value-stdin)
```

#### `conflict list`

Lists unresolved conflicts, optionally filtered by project and field. `--limit` truncates results. Output includes variant tokens needed by `conflict resolve --use`.

#### `conflict show`

Shows every unresolved conflict for a task or only the selected field. Values for project and label fields are rendered as user-facing keys and names rather than internal IDs.

#### `conflict diff`

Prints the local and remote values for a single conflict as a text diff. It requires exactly one unresolved conflict for the task and field.

#### `conflict export`

Creates the destination directory and writes each variant to `<field>-<variant>.md` for external inspection or merging.

#### `conflict resolve`

Resolve with a variant token from `list` or `show`, or provide a replacement value through exactly one inline, file, or standard-input source. `--use` takes precedence when supplied, so do not combine it with a value source.

```sh
aven conflict list --project aven
aven conflict show APP-7KQ9 --field description
aven conflict diff APP-7KQ9 description
aven conflict export APP-7KQ9 description --dir ./conflict-review
aven conflict resolve APP-7KQ9 description --use a
aven conflict resolve APP-7KQ9 description --value-file ./merged.md
```

### `aven daemon`

Run synchronization continuously or manage the macOS LaunchAgent service.

```sh
aven daemon
aven daemon install [--program <path>]
aven daemon uninstall
aven daemon restart
aven daemon repair [--if-installed] [--program <path>]
```

Running `aven daemon` in the foreground requires `sync.enabled: true` and `sync.server_url`. It syncs immediately, then on the configured interval and whenever a local mutation sends a UDP wake signal. Failed syncs use exponential backoff up to five minutes. Bounded rounds with more work remaining resume promptly. The process exits when its executable changes so a service manager can restart the updated binary.

Service management is available on macOS:

- `install` writes, enables, and loads the LaunchAgent. It records the resolved database, config directory, executable, wake address, and log paths. `--program` stores an explicit executable path instead of the running binary.
- `uninstall` unloads and removes the LaunchAgent plist.
- `restart` asks `launchctl` to restart the loaded service.
- `repair` rewrites and reloads an existing LaunchAgent from active configuration. `--if-installed` succeeds without changes when the service is absent. `--program` selects the stored executable path.

```sh
aven daemon
aven daemon install
aven daemon repair --if-installed
aven daemon restart
```

## Interactive commands

### `aven demo`

Open the TUI with a fresh temporary database containing the curated sample
projects, labels, tasks, dependencies, and epic membership used in product
screenshots.

```sh
aven demo
```

### `aven tui`

Open the terminal user interface.

```sh
aven tui [<task-ref>] [--view <view>] [-p [<project>] | --project [<project>]] [--label <label>] [--priority <priority>] [--add-task | --add-task-only] [--natural]
```

| Argument or option | Description |
| --- | --- |
| `[<task-ref>]` | Open a task detail directly. Task refs cannot be combined with browse context or composer options. |
| `--view <view>` | Start in `queue`, `columns`, `open`, `inbox`, `active`, `backlog`, `todo`, `done`, `upcoming`, `conflicts`, `epics`, or `recent-actions`. |
| `-p`, `--project [<project>]` | Start in a project. Passing the flag without a value uses the project inferred from the current directory. Omitting the flag starts with workspace scope. |
| `--label <label>` | Apply an initial label filter. |
| `--priority <priority>` | Apply an initial `none`, `low`, `medium`, `high`, or `urgent` priority filter. |
| `--add-task` | Open the add-task composer over the selected browse context. |
| `--add-task-only` | Run only the add-task popup and exit after submission or cancellation. Project scope remains available. |
| `--natural` | Use natural-language intake. Requires `--add-task` or `--add-task-only`. |

Views, project scope, label, priority, and `--add-task` compose. The `recent-actions` view rejects label and priority filters because it displays change-log entries rather than tasks. `--add-task` and `--add-task-only` are mutually exclusive.

```sh
aven tui
aven tui --project aven --view todo --label bug
aven tui --view columns
aven tui AVN-7KQ9
aven tui --add-task --natural
aven tui --project aven --add-task-only
```

## Agent commands

### `aven prime`

Emit an agent primer plus open work for an inferred or selected project.

```sh
aven prime [--project <project>] [--limit <number>] [--json]
```

Without `--project`, aven infers the project from the current directory. Text output includes CLI conventions, an issue workflow, actionable task-title guidance, sampled status and label frequencies, top blockers, and open tasks partitioned into Active, Ready, and Blocked. Epic containers and done or canceled tasks are excluded.

`--json` emits project context, conventions, top blockers, and the same task partitions. `--limit` bounds the task sample in both text and JSON output.

```sh
aven prime
aven prime --project aven
aven prime --project aven --limit 20 --json
```

### `aven skill`

Print or install aven's coding-agent skill.

```sh
aven skill
aven skill install [--agent <agent>]...
```

With no subcommand, `aven skill` prints the embedded skill Markdown to standard output. `skill install` writes the skill into detected coding-agent skill directories. `--agent` is repeatable and accepts `claude`, `opencode`, or `codex`. Without explicit targets, aven installs to all detected supported agents.

```sh
aven skill > /tmp/aven-skill.md
aven skill install
aven skill install --agent claude --agent codex
```

## Setup commands

### `aven config`

Create or inspect the config file.

```sh
aven config init
aven config show
aven config get sync.enabled
```

`config init` creates the default `config.yaml` and fails rather than overwriting an existing file. `config show` prints the resolved config path followed by its stored text. `config get sync.enabled` prints the resolved Boolean value. The config directory comes from `AVEN_CONFIG_DIR` or the platform default.

```sh
aven config init
aven config show
aven config get sync.enabled
```

### `aven doctor`

Diagnose active configuration, routing, storage, sync, and daemon state.

```sh
aven doctor [--integrity] [--json]
```

The report includes config and database paths and their sources, current directory, workspace and project routing, database and workspace counts, client and sequence metadata, sync configuration and recent sync state, unresolved conflict count, daemon wake settings, and macOS service status. Attachment checks report image storage, cleanup eligibility, quotas, operations in progress, and inconsistencies.

Normal doctor checks attachment records and local image availability. `--integrity` also runs SQLite and relationship checks across the database, verifies recurring schedules, generated tasks, history, pauses, and active, paused, or stopped state, verifies stored image hashes and sizes, decodes the images, and confirms their formats and dimensions. A missing current recurring task is a repairable warning with guidance to run `aven recur list`. Other recurring-task integrity failures include guidance to preserve the database and recover from a known-good backup. `--json` preserves each result as a labeled row with `ok`, `warning`, `error`, or `info` status.

```sh
aven doctor
aven doctor --integrity
aven doctor --json
```

### `aven update`

Check GitHub releases for a newer aven version.

```sh
aven update [--yes]
```

Package-manager installations receive manager-specific update instructions. Direct installations report an available release without changing the executable unless `--yes` is supplied. A direct update downloads the platform archive, verifies it, replaces the current executable, and asks you to restart running aven processes. Cached release information is used when a fresh check fails and a valid cache entry exists.

```sh
aven update
aven update --yes
```

## Data safety commands

### `aven backup`

Create or restore a compressed backup archive.

```sh
aven backup [--output <path>]
aven backup restore <path> --yes
```

Without `--output`, backup creates a timestamped `.aven-backup.tar.zst` archive beside the active database. The archive contains a consistent database backup and every attachment image available on this device. The database preserves recurring schedules, future-task settings, completed, skipped, and missed history, pauses, conflicts, and task-specific notes and attachments. Attachment records for missing images remain in the database, but the missing files cannot be included. Cached previews and incomplete files are excluded. Aven validates every included image while creating the archive.

`backup restore` requires `--yes`. Restore checks the archive, database, attachment information, and image files before replacing local data. It creates safety copies of the existing database and attachment directory first. Plain SQLite backup files remain accepted for database-only recovery.

```sh
aven backup
aven backup --output ~/backups/aven.aven-backup.tar.zst
aven backup restore ~/backups/aven.aven-backup.tar.zst --yes
```

### `aven export`

Export portable user and sync metadata as JSON.

```sh
aven export --output <path>
```

The export includes tasks, recurring schedules, future-task settings, history, pauses, conflicts, and attachment information. Past dated tasks remain ordinary task rows. Image files are excluded and `blobs_included` is `false`. Parent directories are created as needed. The command reports workspace and task counts and output size.

```sh
aven export --output ~/backups/aven.json
```

### `aven import`

Replace local data from an aven JSON export.

```sh
aven import <path> --yes
```

Import requires explicit confirmation with `--yes`. Aven validates the export, relationships, attachment information, recurring schedules, generated tasks, history, pauses, and active, paused, or stopped state before replacing local data. It creates a safety backup, preserves this installation's client identity, clears server-specific sync state, and runs integrity checks. Past dated tasks keep their own fields, notes, and attachment metadata. Because JSON contains no image files, imported attachments are marked unavailable until sync or a backup restore supplies them.

Exports without recurrence sections import every task as nonrecurring data.

```sh
aven import ~/backups/aven.json --yes
```
