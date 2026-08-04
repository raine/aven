---
title: Configuration
description: Configure storage, routing, sync, update checks, TUI columns, and agent intake.
---

aven works without a config file. Add one when defaults are not enough.

```sh
aven config init
```

aven reads `config.yaml` from `AVEN_CONFIG_DIR` when set, otherwise from `~/.config/aven`.

Use `aven doctor` to inspect the active config, database path, workspace, project, sync cursor, daemon wake address, and routing decisions.

## Config shape

```yaml
local:
  db_path: "/path/to/aven.sqlite"
  blob_dir: "/path/to/aven-blobs"
  inline_images: auto
  image_optimization: off
  attachment_lifecycle:
    grace_days: 7
    server_grace_days: 30
    quota_bytes: 10737418240
    server_workspace_quota_bytes: 10737418240
    preview_quota_bytes: 536870912
    maintenance_limit: 128

workspace:
  default: "personal"
  routes:
    - workspace: "work"
      paths: ["~/work"]

project:
  overrides:
    - project: "aven"
      paths: ["~/code/aven"]

sync:
  enabled: true
  server_url: "http://127.0.0.1:3000"
  auth_token: "shared-secret"
  interval_seconds: 30

daemon:
  wake_addr: "127.0.0.1:47631"

update:
  automatic_checks: true

tui:
  columns:
    - name: "Inbox"
      statuses: [inbox]
    - name: "Backlog"
      statuses: [backlog]
    - name: "Todo"
      statuses: [todo]
    - name: "Active"
      statuses: [active]
    - name: "Done"
      statuses: [done, canceled]

agent:
  task_intake:
    command: "claude"
    args: ["-p", "--no-session-persistence", "--bare", "{prompt}"]
    timeout_seconds: 45
```

## Database path

Tasks live in SQLite. aven resolves the database path from `--db`, then `AVEN_DB`, then `local.db_path`, then the default state directory.

Use `local.db_path` when you want an explicit database location:

```yaml
local:
  db_path: "~/tasks/aven.sqlite"
```

## Attachment lifecycle

All image attachment settings are optional. The defaults support normal use without configuration.

### Storage location

`local.blob_dir` selects the directory that stores attachment image files. Absolute paths are used as written. Relative paths are resolved beside the active database. When omitted, Aven creates a sidecar directory beside the database. Use `aven backup` when copying Aven data because copying only the SQLite database does not include these image files.

### Image previews

`local.inline_images` controls whether the TUI draws image previews or shows text labels. Locally available images remain focusable and can open in the operating system viewer in every mode:

| Value | Behavior |
| --- | --- |
| `off` | Always show text labels without inline previews. |
| `auto` | Show previews in supported terminals outside tmux and labels everywhere else. This is the default. |
| `on` | Show previews in supported terminals and enable tmux passthrough. |

Aven uses the iTerm2 inline-image protocol in iTerm2 and the Kitty graphics protocol in Kitty, WezTerm, and Ghostty. Setting `on` helps these protocols pass through tmux, but cannot add image support to another terminal. Sixel-only terminals show text labels.

Detection uses terminal markers such as `TERM_PROGRAM`, `TERM`, `KITTY_WINDOW_ID`, `WEZTERM_PANE`, and `GHOSTTY_RESOURCES_DIR`. See [Image attachments](/tui/#image-attachments) for controls and [Troubleshoot image previews](/tips/#troubleshoot-image-previews) for setup help.

### PNG optimization

`local.image_optimization` controls lossless PNG optimization:

| Value | Behavior |
| --- | --- |
| `off` | Preserve images unless `attachment add --optimize` overrides it. This is the default. |
| `paste` | Optimize pasted images and preserve CLI file attachments. |
| `on` | Optimize pasted images and CLI file attachments unless `--no-optimize` overrides it. |

Optimization applies only to PNG files. Aven preserves the original when optimization fails or does not reduce the file size, and validates any optimized file before storage. Cached previews are disposable, regenerate when needed, and are excluded from sync, backup, export, and import.

### Retention and storage limits

| Setting | Default | Purpose |
| --- | --- | --- |
| `grace_days` | 7 days | Minimum age before an unused local image becomes eligible for cleanup. |
| `server_grace_days` | 30 days | Minimum age before an unused server image becomes eligible for cleanup. |
| `quota_bytes` | 10 GiB | Maximum unique attachment image storage on one device. |
| `server_workspace_quota_bytes` | 10 GiB | Maximum unique attachment image storage for one server workspace. |
| `preview_quota_bytes` | 512 MiB | Maximum disposable preview-cache size. |
| `maintenance_limit` | 128 | Maximum files processed in one cleanup run. |

Images used by tasks or attachment operations in progress are protected from cleanup. Identical images share storage and count once toward attachment quotas. The preview cache uses its separate quota and does not count toward image-file quotas. `aven attachment prune` performs a dry run unless you pass `--apply`.

## Workspace routes

A workspace is a task universe. Workspace routes choose the active workspace from the current directory.

```yaml
workspace:
  default: "personal"
  routes:
    - workspace: "work"
      paths: ["~/work"]
```

A `--workspace` flag overrides route inference for one command.

## Project path mappings

Projects commonly map to repositories or directories. By default, aven infers the project from the current repository or directory name.

Project overrides make that inference explicit when a path should belong to a specific project:

```yaml
project:
  overrides:
    - project: "aven"
      paths: ["~/code/aven"]
```

## Sync and daemon settings

Sync is optional and self-hosted. Configure sync when `aven sync` and `aven daemon` should use a default server:

```yaml
sync:
  enabled: true
  server_url: "http://127.0.0.1:3000"
  auth_token: "shared-secret"
  interval_seconds: 30

daemon:
  wake_addr: "127.0.0.1:47631"
```

:::note[Loopback wake address]
Daemon wake addresses must be loopback.
:::

## Automatic update checks

The TUI checks for releases in the background and shows an update badge when a release is available. Set `automatic_checks` to `false` to disable these automatic checks and notifications:

```yaml
update:
  automatic_checks: false
```

Set `AVEN_NO_UPDATE_CHECK=1` to disable automatic checks for one environment. The environment variable takes precedence over `update.automatic_checks`. `aven update` and the TUI's explicit update command are available with either setting.

## TUI columns

The Columns view groups Aven's semantic statuses into named lanes. Names and order are presentation settings. Task status values remain `inbox`, `backlog`, `todo`, `active`, `done`, and `canceled` across the CLI, sync, queue, dependencies, and agent workflows.

The default board keeps every status visible:

```yaml
tui:
  columns:
    - name: "Inbox"
      statuses: [inbox]
    - name: "Backlog"
      statuses: [backlog]
    - name: "Todo"
      statuses: [todo]
    - name: "Active"
      statuses: [active]
    - name: "Done"
      statuses: [done, canceled]
```

Lanes use workflow-specific ordering. Inbox shows the oldest tasks first, Backlog and Todo sort by priority and then age, Active shows the stalest activity first, and Done shows the most recently completed or canceled tasks first. Custom lanes that combine statuses from different workflow stages preserve the active task-list order.

Customize lane names, order, and grouping by editing this list. Each fixed status must appear exactly once. Aven rejects empty columns, unknown statuses, duplicates, and incomplete mappings so the board cannot hide tasks accidentally.

The first status in each lane is its movement destination. For example, moving a task into the default Done lane sets its status to `done`. Choosing the lane a task already occupies preserves its existing status, including `canceled` within Done.

## Agent task intake

Natural-language task intake can call an external agent command. The configured command receives a prompt through the `{prompt}` argument placeholder. Custom system prompts can use `{input}`, `{priorities}`, `{selected_project}`, `{inferred_project}`, `{projects}`, and `{labels}`. A non-empty selected project is authoritative, while the inferred project comes from current-directory routing.

```yaml
agent:
  task_intake:
    command: "claude"
    args: ["-p", "--no-session-persistence", "--bare", "{prompt}"]
    timeout_seconds: 45
```

## Environment overrides

Useful environment overrides include:

| Variable | Purpose |
| --- | --- |
| `AVEN_CONFIG_DIR` | Config directory containing `config.yaml` |
| `AVEN_DB` | SQLite database path |
| `AVEN_SYNC_SERVER` | Sync server URL |
| `AVEN_NO_UPDATE_CHECK` | Disable automatic update checks and TUI notifications when set to `1`, `true`, or `yes` |
