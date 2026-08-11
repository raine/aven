---
title: Custom TUI commands
description: Add local programs to the command palette and receive task context as JSON.
---

:::caution[Experimental feature]
Custom TUI commands are experimental and may change or be removed in a future
release.
:::

Custom TUI commands run local executables from Aven's command palette. They can
pass along the selected task and marked tasks, refresh the TUI after changing
data, or close Aven when the program succeeds.

You might use one to:

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
      cwd: ~/code/release-tools
      env:
        EXPORT_PROFILE: staging
        NO_COLOR: "1"
      timeout_seconds: 30
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
| `cwd` | No | Aven's invocation directory | Child working directory. A leading `~` expands to the home directory, and relative paths resolve from Aven's invocation directory. |
| `env` | No | `{}` | Static environment overrides added to Aven's inherited environment. |
| `timeout_seconds` | No | `300` for `wait`, unlimited for `terminal` | Complete operation deadline from 1 through 86,400 seconds. Background input handoff keeps its fixed short deadline. |
| `args` | No | `[]` | Static arguments passed directly to the executable. |
| `keys` | No | `[]` | Key sequences available in task lists and inherited by task detail. |
| `detail_keys` | No | `keys` | Key sequences for task detail. An empty list disables detail bindings. |
| `target` | No | `focused` | Operational target policy. Values are `none`, `focused`, `marked`, and `marked-or-focused`. |
| `execution` | No | `wait` | Process mode. Values are `wait`, `background`, and `terminal`. |
| `on_success` | No | `stay` | Behavior after success. Values are `stay`, `refresh`, `quit`, and `refresh-and-quit`. |

Names and aliases may contain lowercase ASCII letters, digits, and hyphens.
Each one must be unique across custom and built-in commands. The config loader
reports blank descriptions or programs, unknown fields, duplicate names, and
unsupported lifecycle combinations.

Environment variable names cannot be empty or contain `=` or NUL, and values
cannot contain NUL. At launch, `cwd` must point to an existing directory.

Commands inherit the current environment, with configured values overriding
variables of the same name. Values are passed through without interpolation and
stay out of invocation JSON, diagnostics, and logs.

The program runs directly with the configured `args`. Pipelines, redirection,
variable expansion, and other shell behavior belong in an executable script.

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
dispatch or `z e` for export. The `z` prefix is reserved for configured commands,
so built-in commands do not use it. Other unassigned keys work too, but `z`
avoids collisions with present and future built-ins.

A sequence may contain up to four keys. Each token can be one Unicode character
or one of these names: `Space`, `Enter`, `Backspace`, `Tab`, `Shift+Tab`,
`Home`, `End`, `Up`, `Down`, `Left`, `Right`, `PageUp`, `PageDown`, `Delete`,
`Insert`, and `F1` through `F12`.

Exact collisions with built-in or custom bindings in the same view are invalid.
Prefix overlap is supported, so a command can extend an existing prefix. `Esc`
and `?` are reserved by TUI input handling. `Up`, `Down`, `PageUp`, and
`PageDown` scroll the prefix menu and cannot follow a prefix.

## Task selection and targets

The `target` policy defines the task identities on which the command operates:

| Policy | Availability | Resolved targets |
| --- | --- | --- |
| `none` | Always | Empty |
| `focused` | A primary task exists | The primary task |
| `marked` | At least one marked task exists | Marked tasks in visible order |
| `marked-or-focused` | A mark or primary task exists | Marked tasks when present, otherwise the primary task |

The palette shows the number of marked targets for batch commands. A `marked`
command still appears when no tasks are marked, but its disabled reason says
`requires one or more marked tasks`. Keybindings follow the same availability
and targeting rules as the palette.

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

Every invocation gets one versioned JSON document. `wait` and `background`
commands read it from standard input. The input pipe closes after the full
document is written. `terminal` commands read it from a protected temporary file
whose path is in `AVEN_COMMAND_CONTEXT`. Task data never appears in arguments or
a shell command.

```json
{
  "version": 1,
  "command": {
    "name": "dispatch",
    "invoked_as": "custom-dispatch"
  },
  "invocation": {
    "cwd": "/Users/example/code/release-tools",
    "origin_cwd": "/Users/example/code/project",
    "tui_pid": 12345,
    "aven_exe": "/Users/example/bin/aven",
    "config_dir": "/Users/example/.config/aven",
    "db_path": "/Users/example/.local/state/aven/db.sqlite",
    "blob_dir": "/Users/example/.local/state/aven/db.sqlite.blobs"
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
| `invocation.cwd` | Effective child working directory after command-specific resolution. |
| `invocation.origin_cwd` | Aven's invocation working directory before a command-specific override. |
| `invocation.tui_pid` | Process ID of the originating Aven TUI. |
| `invocation.aven_exe` | Running Aven executable path, or `null` when the platform cannot resolve it. |
| `invocation.config_dir` | Resolved active configuration directory, or `null` when unavailable. |
| `invocation.db_path` | Active file database path, or `null` when unavailable. |
| `invocation.blob_dir` | Active local attachment blob directory, or `null` when unavailable. |
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

Each task in `selection.primary` or `selection.marked` includes:

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
and configuration secrets. Invocation paths are local metadata sent only to the
configured program. A command can use explicit CLI flags to call the same Aven
executable and database:

```bash
aven_exe=$(jq -r '.invocation.aven_exe' "$input")
db_path=$(jq -r '.invocation.db_path' "$input")
"$aven_exe" --db "$db_path" show "$ref" --json
```

Check for `null` before using a platform path.

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

Read standard input before starting a long-lived child. The pipe closes after the
complete document is written.

## Execution modes

### Wait

`execution: wait` runs the program without blocking the TUI:

1. Delivers JSON input while concurrently draining standard output and standard
   error.
2. Closes standard input after delivering the complete document.
3. Waits for the process and all pipe workers to finish.
4. Treats exit code zero as success.
5. Reports a nonzero exit code, signal termination, timeout, or input failure.

One deadline covers the whole operation: input delivery, output draining,
process completion, timeout cleanup, and reaping the direct child. The default
is five minutes. Set `timeout_seconds` to a value from 1 through 86,400 to use a
different deadline.

The runner keeps the last 16 KiB from each output stream while it continues
draining both. Writing more than 16 KiB does not turn a successful command into
a failure. For a nonzero exit, the error message shows a short, sanitized
excerpt from standard error, or from standard output when standard error is
empty.

On Unix, a timeout or orderly TUI shutdown terminates the command's process
group, including descendants. Other platforms terminate only the direct child;
process-tree cleanup is unavailable there.

Use `wait` when the TUI needs to know whether the program succeeded, especially
with `on_success: quit`.

### Background

`execution: background` starts the program, delivers its complete JSON input
within a short bounded handoff, closes standard input, and returns without
waiting for process completion. `timeout_seconds` does not replace this fixed
input-handoff deadline. Standard output and standard error are discarded. On
Unix, the child runs in a separate process group. A failed or
timed-out handoff terminates that group, while a successful handoff remains
independent of TUI shutdown.

Background mode confirms launch and input delivery, not the final outcome. The
TUI stays open, so background commands require `on_success: stay`. The
`refresh`, `quit`, and `refresh-and-quit` policies are invalid because process
completion is not observed.

Use `background` for long-lived programs whose result does not control the TUI
lifecycle.

### Terminal

`execution: terminal` suspends the TUI and gives the child the terminal's input,
output, and error streams. Use it for editors, pagers, fuzzy finders, interactive
agents, and other full-screen programs. The command runs directly with its
configured arguments, working directory, and environment. The TUI waits for it
to exit.

Terminal commands receive JSON through the file named by
`AVEN_COMMAND_CONTEXT`:

```bash
context=${AVEN_COMMAND_CONTEXT:?AVEN_COMMAND_CONTEXT is required}
ref=$(jq -r '.selection.primary.ref' "$context")
```

The context file is flushed and closed before launch. On Unix, it has owner-only
permissions. It stays in place until the child exits, then is removed after the
terminal is restored. Diagnostics and logs never include the path.

Terminal commands have no timeout by default. When `timeout_seconds` is set, a
timeout terminates the process tree on Unix, waits for cleanup, restores the TUI,
and reports the error. Other platforms terminate the direct child.

The terminal is restored before the success policy runs. Restoration covers raw
mode, the alternate screen, keyboard enhancements, mouse and bracketed-paste
state, cursor state, and inline images, followed by a full redraw. Child output
is not captured. Whether it remains in scrollback depends on the terminal
emulator.

On Linux with Kitty, terminal commands use a foreground process group. Ctrl-C
reaches the child without terminating the TUI, and an interrupted child reports
a nonzero status when it exits. Terminal-size changes reach the child's TTY, and
the TUI redraws at the new size afterward. Runtime coverage for this behavior is
limited to Linux with Kitty.

## Success policies

| Policy | Behavior |
| --- | --- |
| `stay` | Show completion and retain the current projection. This is the default. |
| `refresh` | Refresh the active workspace projection, preserve navigation where valid, then show completion. |
| `quit` | Perform orderly TUI shutdown after a waiting or terminal command exits successfully. |
| `refresh-and-quit` | Refresh application state, then perform orderly TUI shutdown. |

Refresh follows the normal committed-projection path. It keeps the active view
and filters, and restores list selection by task identity when possible. Detail
stays bound to the displayed task while that task is available. Marks are
reconciled against the refreshed projection.

A child failure does not refresh or close the TUI. If the child succeeds but the
refresh fails, the TUI stays open and shows a bounded error. A failed refresh
also prevents `refresh-and-quit` from shutting down.

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
new window and exits successfully. The original Aven pane then shuts down. If
tmux returns an error, the TUI stays open and shows the failure.

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

### The TUI remains open

- `quit` and `refresh-and-quit` apply to `execution: wait` and
  `execution: terminal`.
- The program must exit with status zero.
- `refresh-and-quit` requires both child success and a successful refresh.
- A background command uses `on_success: stay` and never requests TUI shutdown.

### A failure has no diagnostic excerpt

For a nonzero exit, diagnostics use standard error and fall back to standard
output only when standard error is empty. The program should write a concise
failure reason to one of those streams before it exits. Output from successful
commands is ignored.

### The program needs shell syntax

Custom commands do not run through a shell. Put shell syntax in an executable
script and set `program` to that script. This keeps task text in JSON instead of
shell source.

## Security

Custom commands run with the same operating-system identity and inherited
environment as the TUI. Treat configured programs as trusted code.

Keep these boundaries in scripts:

- Parse the delivered context as JSON, from standard input for `wait` and
  `background` or from `AVEN_COMMAND_CONTEXT` for `terminal`.
- Quote every value used in shell commands.
- Do not evaluate task titles, descriptions, notes, labels, or other task text.
- Keep secrets in config-level static environment overrides, never in task data.
- Avoid printing configured environment values. Bounded child diagnostics may
  display them.
- Prefer direct argv calls over constructing shell command strings.
- Store scripts in locations writable only by trusted users.
- Avoid logging the complete JSON document when task content is sensitive.
