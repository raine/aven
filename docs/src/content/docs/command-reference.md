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

The `--json` option is available on `context`, `show`, `list`, `search`, `attachment add`, `attachment list`, `attachment get`, `attachment delete`, `attachment prune`, `dep list`, `epic list`, `prime`, `label list`, `project list`, `conflict list`, `conflict show`, and `doctor`. JSON task objects expose temporal fields through `available_at` and `due_on`, and epic membership separately from dependency ordering through `is_epic`, `epic_parent`, and `epic_children`. `context --json` and `show --full --json` also expose attachment metadata without bytes. An empty `available_at` means immediate availability. An empty `due_on` means no deadline.

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
| `--label <label>` | Add a label. Repeat for multiple labels. Labels must already exist. |
| `--epic` | Create an epic container. |
| `--natural` | Parse the title as natural-language task intake using the configured agent command. |

A plain task starts with status `inbox`. `--natural` cannot be combined with a description source, `--project`, a non-default priority, `--label`, `--available-at`, or `--due`. Natural intake can infer a title, description, project, status, priority, labels, availability, due date, and epic state from the request.

The command prints the created task's qualified reference and bare suffix.

```sh
aven add "Fix conflict display" --project aven --priority high --label bug
aven add "Review launch notes" --available-at "next monday at 9am"
aven add "Submit expense report" --due "next fri"
aven add "Add release automation" --epic
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

Search task references, titles, descriptions, projects, labels, notes, statuses, and priorities across the active workspace. Done and canceled tasks participate in normal search. Deleted tasks require `--all`.

```sh
aven search <query>... [--limit <number>] [--all] [--json]
```

| Argument or option | Description |
| --- | --- |
| `<query>...` | One or more search terms. Multiple shell arguments are joined with spaces. |
| `--limit <number>` | Maximum result count. Defaults to `50`. |
| `--all` | Include deleted tasks. |
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

The snapshot includes stable identity, display reference, task fields, description, project and workspace metadata, labels, notes, dependencies, dependents, epic membership, deletion state, and unresolved conflicts. It also derives whether the task is blocked, has conflicts, or blocks open work.

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

### `aven attachment`

Manage task image attachments without changing task descriptions.

```sh
aven attachment add <task-ref> <path> [--alt <text>] [--filename <name>] [--media-type <type>] [--optimize | --no-optimize] [--json]
aven attachment list <task-ref> [--all] [--json]
aven attachment get <attachment-id> [--output <path>] [--all] [--json]
aven attachment delete <attachment-id> [--json]
aven attachment prune [--dry-run | --apply] [--json]
```

Attachment metadata determines identity, display order, and search matches. Attachment IDs are exact, workspace-scoped 16-character uppercase Crockford Base32 identifiers. Task-ref suffix and normalization rules do not apply. Live attachments appear in `(created_at, attachment_id)` order. Attachment `list` and `get` require `--all` to include tombstoned metadata. Human task sections render live attachments only, while `aven context --json` and `aven show --full --json` include tombstones. Compact `aven show --json` has no attachment array.

Attachment command JSON exposes `attachment_id`, `task_id`, `sha256`, `byte_size`, `media_type`, `filename`, `alt_text`, derived dimensions, timestamps, deletion state, and `has_blob`. Add output also contains `optimized`. Task attachment objects from `context --json` and `show --full --json` omit `sha256` and expose the other metadata fields. Search matches live attachment filenames and alternative text.

Aven accepts nonempty decoded PNG, JPEG, GIF, and WebP images up to 25 MiB of encoded data. Either edge can be at most 16,384 pixels, each frame can contain at most 40,000,000 pixels, and an animation can contain at most 100 frames. Filenames must contain 1-255 UTF-8 bytes with no control characters or path separators. Alternative text can contain at most 500 UTF-8 bytes and no control characters. Aven validates complete decoded content and any declared media type before storing bytes. Dimensions always come from decoded content. `--optimize` requests lossless PNG optimization, while `--no-optimize` preserves file bytes. Without either flag, `local.image_optimization` controls file attachments. Optimization uses the validated lossless PNG only when it is smaller than the input.

Attachment deletion is an idempotent tombstone operation, and there is no attachment restore command. Deleting an attachment or its task removes the live reference. After the grace period, pruning can make a later task restore metadata-only unless another live reference, the server, or a backup retains the bytes.

`attachment prune` reports one batch of eligible unique-object counts and bytes without exposing attachment identifiers, filenames, hashes, or paths. The batch is capped by `local.attachment_lifecycle.maintenance_limit`, so repeated runs may be needed. Original-object deletion is a dry run unless `--apply` is passed. Preview-cache eviction to `preview_quota_bytes` runs in either mode because previews are disposable. Live references, unsynced adds, staging operations, upload reservations, active transfers, reads, and backups protect original objects from pruning. Original-object quotas count each physical hash once locally and each hash once per server workspace. Preview cache bytes use an independent disposable-cache limit.

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

Output reports pushed and pulled change counts, uploaded and downloaded attachment blob counts and byte totals, remaining blob counts and bytes, the resulting server cursor, and completion state. Attachment transfers run in bounded rounds of up to 16 unique blobs and 64 MiB across uploads and downloads.

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

## Interactive command

### `aven tui`

Open the terminal user interface.

```sh
aven tui [-p [<project>] | --project [<project>]] [--add-task] [--add-task-only] [--natural]
```

| Option | Description |
| --- | --- |
| `-p`, `--project [<project>]` | Start in a project. Passing the flag without a value uses the project inferred from the current directory. Omitting the flag starts with workspace scope. |
| `--add-task` | Open the add-task composer when the full TUI starts. |
| `--add-task-only` | Run only the add-task popup and exit after submission or cancellation. |
| `--natural` | Use natural-language intake when the startup add-task composer opens. |

`--natural` has an effect with `--add-task` or `--add-task-only`. `--add-task-only` takes precedence over the full TUI flow.

```sh
aven tui
aven tui --project aven
aven tui -p
aven tui --add-task --natural
aven tui --add-task-only
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

The report includes config and database paths and their sources, current directory, workspace and project routing, database and workspace counts, client and sequence metadata, sync configuration and recent sync state, unresolved conflict count, daemon wake settings, and macOS service status. Attachment lifecycle rows report referenced, protected, grace-period, eligible, staging, trash, quota, reservation, and inconsistency counts and bytes.

Normal doctor checks attachment inventory, task and change links, inventory metadata, supported media types, positive sizes, missing available objects, and orphan object files. `--integrity` also runs SQLite quick-check and referential consistency checks across task, project, path, label, note, dependency, epic, conflict, field-version, and metadata records. Its deep attachment checks verify object hashes, encoded sizes, decoded image content, media types, and stored dimensions. `--json` preserves each result as a labeled row with `ok`, `error`, or `info` status.

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

Without `--output`, backup creates a timestamped `.aven-backup.tar.zst` archive beside the active database. The archive contains a consistent SQLite backup, a manifest, and every locally available original attachment object. The SQLite database retains all blob inventory rows, including unavailable rows. The manifest and object payload contain only inventory rows marked locally available. Staging, preview cache, trash, and incomplete files are excluded. Aven validates each included object's hash, encoded size, media type, and decoded image content while creating the archive.

`backup restore` requires `--yes`. Archive restore validates the entry structure, manifest, SQLite database, object set, hashes, image content, and canonical attachment metadata before replacement. It replaces the database and original-object set, creates SQLite and blob-directory safety copies, and excludes disposable or incomplete directories from the restored sidecar. Plain SQLite backup files remain accepted for database-only recovery.

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

The versioned export includes workspaces, projects, project paths and aliases, labels, tasks, epic links, task labels, notes, dependencies, attachment metadata, blob inventory, changes, field versions, conflicts, and portable metadata. Attachment bytes are excluded and the top-level `blobs_included` field is `false`. Parent directories are created as needed. The command reports workspace and task counts and output size.

```sh
aven export --output ~/backups/aven.json
```

### `aven import`

Replace local data from an aven JSON export.

```sh
aven import <path> --yes
```

Import requires explicit confirmation with `--yes`. Aven validates the export format, version, schema version, relationships, attachment IDs, hashes, media types, sizes, filenames, alternative text, dimensions, and inventory consistency. It creates a safety backup, replaces data in one transaction, preserves the target installation's client identity, resets server-specific sync state, and runs integrity checks before committing. Imported blob inventory is marked unavailable because JSON contains no bytes, and pending imported attachment operations do not create upload obligations.

The export schema version must exactly match the installed aven schema.

```sh
aven import ~/backups/aven.json --yes
```
