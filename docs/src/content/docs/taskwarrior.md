---
title: Taskwarrior comparison
description: How aven differs from Taskwarrior.
---

[Taskwarrior](https://taskwarrior.org/) is a great task manager and a major inspiration for aven. aven exists because I wanted a similar power-user, local-first tool with different defaults for coding-agent workflows, task identity, context, workspaces, and the human interface.

## Different goals

Taskwarrior is built around a powerful CLI. aven is designed so the CLI is agents-first, and the TUI is for humans.

For humans, the TUI is optimized for speed. It starts instantly, supports keyboard-first workflows, and makes common task operations faster than running command sequences.

For agents, the CLI output is compact and explicit. Commands print the task refs and fields agents need to update status, add notes, and leave handoff context.

## Task identity

Taskwarrior's normal task list centers numeric ids. Those ids are positions in the current database view, so after the database changes, the same number can refer to a different task. Taskwarrior has stable UUID metadata, but it is hidden by default and too awkward to use as the everyday way to refer to tasks.

You can configure user-defined attributes (UDAs) to work around that, but aven makes stable task refs the default. Normal output shows refs such as `APP-7KQ9` instead of volatile numeric positions, so a ref copied into a prompt, note, handoff, or command continues to refer to the same task.

aven displays the shortest unique ref it can, similar to how git displays shortened commit hashes.

## Context lives on the task

aven tasks have Markdown descriptions and append-style notes. That makes problem statements, decisions, blockers, and partial progress part of the task record.

Taskwarrior has annotations, but not a first-class place for long Markdown context. The workaround is a sidecar file on the device, with an annotation pointing to it. That is a weak fit for multi-device workflows.

This matters for coding-agent workflows because handoff context should be durable and attached to the work, not scattered across chats, scratch files, or sidecar documents.

## Workspaces are part of the model

aven has first-class workspaces. Personal and work tasks can use the same tool while keeping queues, refs, labels, and projects separate.

Workspace routes can select a workspace based on the current directory, so working under `~/work` can use the work workspace automatically. In Taskwarrior, this kind of directory-aware isolation requires shell wrappers, filters, and custom config around `task` and `taskwarrior-tui`.

## Why build a separate tool?

The gaps above can be worked around with Taskwarrior configuration, UDAs, shell wrappers, filters, sidecar files, and separate TUI tools. aven aims to turn those hacks into a coherent system.

Rather than adding more hacks on top of Taskwarrior, I want to own the full stack and make my own task manager. Before AI coding, building a task manager with aven's scope would perhaps have been an unrealistic side project, but now it is a reasonable one.

## Migrate from Taskwarrior

Taskwarrior installations can differ substantially through custom data locations, included configuration files, user-defined attributes, hooks, and conventions for projects and tags. A coding agent can inspect those choices and propose a migration that fits your setup.

The prompt below uses a two-phase process. The agent first performs read-only discovery and presents a mapping and loss report. It changes aven only after you approve that proposal.

:::caution
A migration through aven's existing CLI is not transactional. It cannot preserve Taskwarrior's original timestamps, and an interrupted migration may require careful cleanup or resumption. Back up both systems, review every unsupported field, and keep aven sync paused until you approve the verification report.
:::

Copy this prompt into a coding agent that can access your Taskwarrior installation and run shell commands:

```text
Migrate my Taskwarrior tasks into aven.

Safety constraints:

- Keep my live Taskwarrior configuration and data unchanged. Taskwarrior hooks,
  garbage collection, recurrence processing, wrappers, and synchronization may
  have side effects, even during discovery or export.
- Resolve the Taskwarrior installation without running commands against live
  data when paths and configuration can be inspected directly. Make a complete
  temporary copy of its configuration and data, disable hooks in the copy, and
  run the installed `task` executable against that copy for export.
- Do not change aven, its configuration, daemon, sync state, workspaces, or
  database during discovery.
- Do not write directly to aven's SQLite database.
- Do not pass Taskwarrior JSON to `aven import`. That command accepts aven's own
  export format and replaces local data.
- Keep task contents, configuration values, credentials, tokens, and hook
  contents local.
- Stop and ask me if these constraints cannot be satisfied.

Discovery:

1. Read `https://aven.raine.dev/llms-full.txt` for aven's system model, CLI
   guidance, installation options, and configuration. Do not initialize, install,
   or change aven during discovery.
2. Resolve the exact `task` command I use, including aliases, shell functions,
   wrappers, executable path, environment variables, and command-line `rc`
   overrides.
3. Determine the installed Taskwarrior version and its effective configuration
   and data locations. Consider `TASKRC`, `TASKDATA`, `~/.taskrc`,
   `$XDG_CONFIG_HOME/task/taskrc`, `data.location`, `~/.task`, and recursively
   included configuration files. Follow the precedence of the installed
   version rather than assuming one fixed layout.
4. Inspect UDA definitions, hooks, contexts, reports, urgency coefficients,
   recurrence settings, and conventions that affect task meaning. Do not
   execute hooks.
5. Create and validate a complete source snapshot. Export every stored task
   from the snapshot with the installed `task` executable. Account for JSON
   arrays and JSON-object streams, and ensure limits, reports, contexts, or
   filters do not truncate the export.
6. Validate the export and report unique UUID counts and counts grouped by
   status, project, priority, and tag. Inspect annotations, dependencies,
   recurrence, dates, and every observed UDA. Use UUIDs, never numeric task ids,
   as source identity.
7. Determine whether aven is installed. If it is absent, include installation
   and initial configuration in the proposal. If it is present, read
   `aven <command> --help` before using each command and run `aven doctor` to
   inspect any existing database, workspace, routing, sync, daemon, and
   pending-change state. Inspect existing workspaces, tasks, projects, labels,
   conflicts, and likely duplicates. Do not create a database just to inspect
   it.

Proposal and approval gate:

Present a concise migration proposal containing:

- Source and target versions and resolved locations. If aven is absent, include
  the documented installation method and proposed initial configuration.
- Export counts and evidence that the export is complete.
- The target aven workspace and whether this is an isolated import or a merge.
- A mapping for every observed Taskwarrior field and UDA.
- Explicit policies for statuses, priorities, projects, tags, annotations,
  dependencies, dates, recurrence, completed tasks, and deleted tasks.
- Normalization collisions, duplicate candidates, dangling or cyclic
  dependencies, and dependencies that would cross aven workspaces.
- Every unsupported, omitted, synthesized, or lossy conversion. Do not migrate
  computed urgency. Explain how unsupported values would remain traceable in
  descriptions or notes when useful.
- How Taskwarrior UUIDs remain traceable and how retries avoid duplicates.
- Expected numbers of tasks, projects, labels, notes, dependencies, and deleted
  or terminal tasks.
- Backup, sync-pausing, rollback, partial-failure, and resume procedures.

Do not make any aven change until I explicitly approve this proposal.

Execution after approval:

1. If aven is absent, install it using the approved documented method. Confirm
   the installed version and read help for each command before using it.
2. Confirm that the source snapshot and any existing aven target still match the
   approved state. Stop if either changed.
3. Pause aven synchronization as approved and create a verified aven backup
   when a target database already exists.
4. Initialize or select the approved target workspace and use an explicit
   workspace on every command instead of relying on directory routing or the
   active workspace.
5. Create prerequisite projects and labels, then tasks and notes. Create
   dependencies in a second pass after every Taskwarrior UUID has an aven ref.
6. Maintain a durable UUID-to-aven-ref mapping file. Use it to detect completed
   work and prevent duplicates when resuming.
7. Stop on the first unexpected error. Do not continue from an uncertain
   partial state.
8. Verify exact counts and mappings, representative records from every mapping
   class, all dependency edges, annotation order, terminal states, duplicate
   absence, and aven database integrity. Confirm that the live Taskwarrior
   configuration and data remain unchanged.
9. Report every deviation from the approved proposal and all remaining manual
   work. Keep aven synchronization paused until I approve the report.
```

Expect the agent to stop after presenting its proposal. Review how it handles unsupported dates and recurrence, custom UDAs, project and label normalization, existing aven tasks, and synchronization before approving the migration.
