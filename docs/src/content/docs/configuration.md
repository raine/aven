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
    - name: "Triage"
      statuses: [inbox]
    - name: "Current"
      statuses: [active]
    - name: "Todo"
      statuses: [todo]
    - name: "Backlog"
      statuses: [backlog]
    - name: "Closed"
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

Daemon wake addresses must be loopback.

## TUI columns

The Columns view groups Aven's semantic statuses into named lanes. Names and order are presentation settings. Task status values remain `inbox`, `backlog`, `todo`, `active`, `done`, and `canceled` across the CLI, sync, queue, dependencies, and agent workflows.

The default board keeps every status visible:

```yaml
tui:
  columns:
    - name: "Triage"
      statuses: [inbox]
    - name: "Current"
      statuses: [active]
    - name: "Todo"
      statuses: [todo]
    - name: "Backlog"
      statuses: [backlog]
    - name: "Closed"
      statuses: [done, canceled]
```

Customize lane names, order, and grouping by editing this list. Each fixed status must appear exactly once. Aven rejects empty columns, unknown statuses, duplicates, and incomplete mappings so the board cannot hide tasks accidentally.

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
