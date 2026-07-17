---
title: Configuration
description: Configure storage, routing, sync, daemon wakeups, TUI columns, and agent intake.
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

Attachment originals use content-addressed storage under `local.blob_dir`, or beside the database when the setting is omitted. `local.attachment_lifecycle` configures retention and capacity:

- `grace_days` controls local retention after the final live reference disappears. The default is 7 days.
- `server_grace_days` controls server retention. The default is 30 days.
- `quota_bytes` limits local unique original bytes. The default is 10 GiB.
- `server_workspace_quota_bytes` limits distinct original hashes per server workspace. The default is 10 GiB.
- `preview_quota_bytes` independently bounds disposable preview cache bytes. Preview bytes do not count toward original quotas.
- `maintenance_limit` bounds objects processed in one maintenance run. The default is 128.

A hash with a live task attachment, pending add, staging operation, reservation, transfer, read, or backup lease remains protected. `aven attachment prune` performs a dry run by default and requires `--apply` for deletion.

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

Natural-language task intake can call an external agent command. The configured command receives a prompt through the `{prompt}` argument placeholder.

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
