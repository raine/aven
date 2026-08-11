---
title: Custom TUI commands
description: Add local programs to the command palette and receive task context as JSON.
---

:::caution[Experimental feature]
Custom TUI commands are experimental and may change or be removed in a future
release.
:::

Custom TUI commands connect Aven's command palette to local executables. A
command can receive the selected task and marked tasks, launch work in another
program, refresh Aven after a successful mutation, and optionally close Aven
after successful completion.

Common uses include:

- Open a task-specific tmux window or session.
- Start a coding agent with the selected task context.
- Send task metadata to a local automation script.
- Generate a task-specific workspace or report.

Custom commands are local configuration. They are not synced between devices.

## Configure a command

Add commands under `tui.commands` in `config.yaml`:

```yaml
tui:
  commands:
    - name: dispatch
      aliases: [custom-dispatch]
      description: Open the selected task in its tmux workspace
      program: ~/bin/dispatch-task
      args: []
      keys: [z d]
      detail_keys: [z D]
      target: focused
      execution: wait
      on_success: quit
```

Open the command palette with `:`, then run `:dispatch` or
`:custom-dispatch`. Configured commands carry a `custom` label in palette
results.

### Configuration fields

| Field | Required | Default | Purpose |
| --- | --- | --- | --- |
| `name` | Yes | | Canonical palette name without the leading `:`. |
| `aliases` | No | `[]` | Additional palette names for the same command. |
| `description` | Yes | | Description shown in palette search results. |
| `program` | Yes | | Executable path or program name. A leading `~` expands to the home directory. |
| `args` | No | `[]` | Static arguments passed directly to the executable. |
| `keys` | No | `[]` | Key sequences available in task lists and inherited by task detail. |
| `detail_keys` | No | `keys` | Key sequences for task detail. An empty list disables detail bindings. |
| `target` | No | `focused` | Operational target policy. Values are `none`, `focused`, `marked`, and `marked-or-focused`. |
| `execution` | No | `wait` | Process supervision mode. Values are `wait` and `background`. |
| `on_success` | No | `stay` | Aven behavior after success. Values are `stay`, `refresh`, `quit`, and `refresh-and-quit`. |

Names and aliases may contain lowercase ASCII letters, digits, and hyphens.
They must be unique across configured and built-in command names. Aven rejects
blank descriptions, blank programs, duplicate names, and unsupported lifecycle
combinations when it loads the configuration.

Aven executes `program` directly and passes `args` without shell evaluation.
Pipelines, redirection, variable expansion, and other shell behavior belong in
an executable script.

## Keybindings

Each entry in `keys` is a case-sensitive sequence separated by spaces. For
example, `keys: [z d, D]` binds both `z` followed by `d` and uppercase `D` in
task lists. The command palette shows configured bindings, and multi-key
bindings use the same prefix hints as built-in commands. Keybinding invocation
sets `command.invoked_as` to the canonical command name.

`detail_keys` controls task-detail bindings. When omitted, detail inherits
`keys`. Set `detail_keys: []` to make a command list-only, or provide a different
list to give detail its own bindings.

### Recommended custom namespace

Use `z` as the first key for custom multi-key bindings, such as `z d` for
dispatch or `z e` for export. Aven reserves the `z` prefix for configured
commands, so built-in commands do not occupy that namespace. Other unassigned
keys are accepted, but `z` is the stable choice for avoiding collisions with
present and future built-ins.

A sequence may contain up to four keys. Each token can be one Unicode character
or one of these names: `Space`, `Enter`, `Backspace`, `Tab`, `Shift+Tab`,
`Home`, `End`, `Up`, `Down`, `Left`, `Right`, `PageUp`, `PageDown`, `Delete`,
`Insert`, and `F1` through `F12`.

Aven rejects exact collisions with built-in or custom bindings in the same
view. Prefix overlap is supported, so a command can extend an existing prefix.
`Esc` and `?` are reserved by TUI input handling. `Up`, `Down`, `PageUp`, and
`PageDown` are reserved after a prefix because they scroll the prefix menu.

## Task selection and targets

The `target` policy defines the task identities on which the command operates:

| Policy | Availability | Resolved targets |
| --- | --- | --- |
| `none` | Always | Empty |
| `focused` | A primary task exists | The primary task |
| `marked` | At least one marked task exists | Marked tasks in visible order |
| `marked-or-focused` | A mark or primary task exists | Marked tasks when present, otherwise the primary task |

The command palette annotates batch targets with their marked count. A `marked`
command remains visible without marks and displays `requires one or more marked
tasks`. Keybindings use the same availability and targeting rules as the palette.

The JSON input keeps raw selection context separate from resolved operational
targets:

- In a task list, the selected task is `selection.primary`.
- In task detail, the displayed task is `selection.primary`.
- Marked tasks appear in `selection.marked` in visible order.
- `targeting.targets` contains the resolved IDs and refs for the configured
  policy.

The primary task does not change when focus moves to a relationship inside task
detail. Marks remain available to custom commands in task detail.

For configuration compatibility, `requires: none` maps to `target: none`, and
`requires: selected-task` maps to `target: focused`. A command cannot supply
both fields. Configurations and examples should use `target`.

## JSON input

Aven writes one versioned JSON document to the program's standard input and
then closes the pipe. Dynamic task data is never interpolated into arguments or
a shell command.

```json
{
  "version": 1,
  "command": {
    "name": "dispatch",
    "invoked_as": "custom-dispatch"
  },
  "invocation": {
    "cwd": "/Users/example/code/project",
    "tui_pid": 12345
  },
  "workspace": {
    "id": "0000000000000000",
    "key": "default",
    "name": "default"
  },
  "targeting": {
    "policy": "focused",
    "resolved_from": "focused",
    "targets": [
      { "id": "7KQ9ABCDE1234567", "ref": "APP-7KQ9" }
    ]
  },
  "selection": {
    "primary": {
      "ref": "APP-7KQ9",
      "task": {
        "id": "7KQ9ABCDE1234567",
        "title": "Review deployment",
        "description": "Check the production rollout.",
        "status": "active",
        "priority": "high",
        "source": "tui",
        "available_at": null,
        "due_on": "2026-08-10",
        "deleted": false,
        "is_epic": false,
        "created_at": "2026-08-05T10:00:00Z",
        "updated_at": "2026-08-06T09:30:00Z"
      },
      "project": {
        "id": "0123456789ABCDEF",
        "key": "app",
        "prefix": "APP"
      },
      "labels": ["release"],
      "notes": [],
      "depends_on": [],
      "blocks": [],
      "epic_parent": null,
      "epic_children": [],
      "recurrence": null,
      "attachments": []
    },
    "marked": []
  }
}
```

### Top-level fields

| Field | Meaning |
| --- | --- |
| `version` | Input schema version. The documented schema uses version `1`. |
| `command.name` | Canonical configured command name. |
| `command.invoked_as` | Name or alias entered in the palette. |
| `invocation.cwd` | Aven's working directory, also used as the child working directory. |
| `invocation.tui_pid` | Process ID of the originating Aven TUI. |
| `workspace` | Active workspace identity and display name. |
| `targeting.policy` | Configured target policy. |
| `targeting.resolved_from` | Resolution source: `none`, `focused`, or `marked`. |
| `targeting.targets` | Ordered durable IDs and display refs for operational targets. |
| `selection.primary` | Complete primary task context or `null`. |
| `selection.marked` | Complete contexts for marked tasks. |

Use `targeting.targets` to decide what the command operates on. Match each target
ID or ref against `selection.primary` and `selection.marked` to find its complete
context. Raw selection remains available even when the configured policy resolves
to a different target set.

### Task context fields

Each primary or marked task contains:

- Display ref and durable task ID.
- Title, description, status, priority, and immutable source.
- Availability, due date, deletion, and epic state.
- Creation and update timestamps.
- Project ID, key, and display prefix.
- Labels and notes.
- Dependencies, dependents, epic parent, and epic children.
- Recurrence identity, schedule, lifecycle, outcome, and projection state when
  applicable.
- Attachment metadata, including local availability, without attachment bytes.

Relationship entries contain task ID, display ref, title, status, priority, and
whether the relationship is unresolved. Attachment entries contain attachment
ID, media type, byte size, optional filename and alt text, dimensions, creation
time, and `has_blob`.

The document contains task content needed by local automation. It excludes
attachment bytes, attachment hashes, sync credentials, authentication tokens,
and configuration secrets.

## Read JSON in a script

A shell script can read the complete document and use `jq` to select fields:

```bash
#!/usr/bin/env bash
set -euo pipefail

input=$(mktemp)
trap 'rm -f "$input"' EXIT
cat > "$input"

ref=$(jq -r '.selection.primary.ref' "$input")
title=$(jq -r '.selection.primary.task.title' "$input")
printf 'Dispatching %s: %s\n' "$ref" "$title"
```

Read standard input before starting a long-lived child. Aven closes the input
pipe after writing the document.

## Execution modes

### Wait

`execution: wait` starts the program asynchronously and keeps the TUI
responsive while it runs. Aven:

1. Delivers JSON input while concurrently draining standard output and standard
   error.
2. Closes standard input after delivering the complete document.
3. Waits for the process and all pipe workers to finish.
4. Treats exit code zero as success.
5. Reports a nonzero exit code, signal termination, timeout, or input failure.

A waiting command has one five-minute deadline covering input delivery, output
draining, process completion, timeout cleanup, and direct-child reaping. Aven
retains the last 16 KiB of standard output and standard error independently
while continuing to drain both streams. Output beyond that retention bound does not
change a successful exit status. For a nonzero exit, Aven shows a sanitized,
bounded standard-error excerpt and falls back to standard output when standard
error is empty. On Unix, timeout and orderly TUI shutdown terminate the
command's process group, including descendants. Other platforms terminate the
direct child because Aven does not yet provide a platform process-tree primitive
there.

Use `wait` when Aven must know whether the operation succeeded, especially with
`on_success: quit`.

### Background

`execution: background` starts the program, delivers its complete JSON input
within a short bounded handoff, closes standard input, and returns without
waiting for process completion. Standard output and standard error are
discarded. On Unix, the child runs in a separate process group. A failed or
timed-out handoff terminates that group, while a successful handoff remains
independent of TUI shutdown.

Background mode confirms launch and input delivery, not the final outcome of
the program. It always leaves Aven running. Background execution requires
`on_success: stay`; `refresh`, `quit`, and `refresh-and-quit` are invalid because
Aven does not observe process completion.

Use `background` for long-lived programs whose result does not control Aven's
lifecycle.

## Success policies

| Policy | Behavior |
| --- | --- |
| `stay` | Show completion and retain the current projection. This is the default. |
| `refresh` | Refresh the active workspace projection, preserve navigation where valid, then show completion. |
| `quit` | Perform orderly TUI shutdown after a waiting command exits successfully. |
| `refresh-and-quit` | Refresh application state, then perform orderly TUI shutdown. |

Refresh uses Aven's normal committed-projection path. It retains the active view,
filters, and list selection by task identity where possible. Detail remains bound
to its displayed task while that task is available, and marks are reconciled
against the refreshed projection.

A child failure never refreshes or closes Aven. If the child succeeds but a
requested refresh fails, Aven remains open and shows a bounded refresh error.
A refresh failure also prevents `refresh-and-quit` from shutting down.

## Tmux dispatch example

This command opens a task-specific tmux window and closes Aven only after tmux
reports success:

```yaml
tui:
  commands:
    - name: dispatch
      aliases: [custom-dispatch]
      description: Open the selected task in a tmux window
      program: ~/.config/aven/commands/dispatch-task
      keys: [z d]
      target: focused
      execution: wait
      on_success: quit
```

Create `~/.config/aven/commands/dispatch-task`:

```bash
#!/usr/bin/env bash
set -euo pipefail

input=$(mktemp)
trap 'rm -f "$input"' EXIT
cat > "$input"

ref=$(jq -r '.selection.primary.ref' "$input")
title=$(jq -r '.selection.primary.task.title' "$input")
window="task-${ref##*-}"

tmux new-window \
  -n "$window" \
  "printf '%s\\n' $(printf '%q' "$title"); exec ${SHELL:-/bin/sh}"
```

Make it executable:

```sh
chmod 755 ~/.config/aven/commands/dispatch-task
```

Run `:dispatch` from a task list or detail view. `tmux new-window` selects the
new window and exits successfully. Aven then shuts down in its original pane.
If tmux returns an error, Aven remains open and reports the failure.

## Multiple commands and aliases

Commands share one palette catalog with built-in actions:

```yaml
tui:
  commands:
    - name: dispatch
      aliases: [agent, custom-dispatch]
      description: Open the selected task in its agent workspace
      program: ~/bin/dispatch-task
      target: focused
      execution: wait
      on_success: quit

    - name: export-context
      description: Save selected task context locally
      program: ~/bin/export-task-context
      target: focused
      execution: wait
      on_success: stay

    - name: dashboard
      description: Open the local dashboard
      program: ~/bin/open-dashboard
      target: none
      execution: background
```

Custom names and aliases participate in the same exact matching, prefix
matching, ambiguity handling, and tab completion as built-in commands.

## Troubleshooting

### The command does not appear

- Confirm it is nested under `tui.commands`.
- Confirm Aven reads the expected `config.yaml`. `AVEN_CONFIG_DIR` selects an
  alternate configuration directory.
- Restart the TUI after editing the configuration.
- Check for invalid or duplicate names in the startup error.
- When testing a development worktree, build and run that worktree's binary.
  Shared Cargo target directories can otherwise contain a binary built from a
  different branch.

### The command is disabled

The palette shows the target requirement that is missing. `target: focused`
needs a selected list task or displayed detail task. `target: marked` needs at
least one mark. `target: marked-or-focused` needs either one. Use `target: none`
only when the program operates without task targets.

### Aven remains open

- `quit` and `refresh-and-quit` apply only to `execution: wait`.
- The program must exit with status zero.
- `refresh-and-quit` requires both child success and a successful refresh.
- A background command uses `on_success: stay` and never requests TUI shutdown.

### A failure has no diagnostic excerpt

Aven uses standard error for nonzero-exit diagnostics and falls back to standard
output only when standard error is empty. Have the program write its concise
failure reason to one of those streams before exiting. Successful command output
is ignored.

### The program needs shell syntax

Aven does not invoke a shell. Put shell syntax in an executable script and set
`program` to that script. This also keeps task text in JSON rather than shell
source.

## Security

Custom commands execute local programs with the same operating-system identity
and inherited environment as Aven. Treat configured programs as trusted code.

Keep these boundaries in scripts:

- Parse stdin as JSON.
- Quote every value used in shell commands.
- Do not evaluate task titles, descriptions, notes, labels, or other task text.
- Prefer direct argv calls over constructing shell command strings.
- Store scripts in locations writable only by trusted users.
- Avoid logging the complete JSON document when task content is sensitive.
