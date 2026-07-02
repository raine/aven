---
title: Concepts
description: Understand the aven task model.
---

## Tasks

A task is the central record in aven. It belongs to a workspace and usually belongs to a project.

Tasks include a title, status, priority, project, labels, Markdown description, notes, relationships, deletion state, epic state, and conflict state.

## Workspaces, projects, and labels

A workspace is a task universe. Use workspaces to isolate personal tasks, work tasks, or any other separate context.

aven resolves the active workspace from `--workspace`, the longest matching workspace route, `workspace.default`, the built-in default, or the only existing workspace.

Workspace routes match the current directory before project inference. Configure routes when paths such as `~/work` or `~/personal` should select different task universes.

A project groups tasks inside a workspace. Projects commonly map to repositories or directories. By default, aven infers the project from the current repository or directory name.

Project path mappings override that inference when a path should belong to a specific project. Use them when a checkout directory has a short name, a generated name, or a name that differs from the project you want in task refs.

```yaml
workspace:
  default: "personal"
  routes:
    - workspace: "work"
      paths: ["~/work"]

project:
  overrides:
    - project: "aven"
      paths: ["~/code/aven"]
```

Use `aven doctor` to inspect the active config, database path, workspace, and project for the current directory. See [Configuration](/configuration/) for the full config shape.

Labels are cross-cutting tags such as `bug`, `docs`, or `ux`. Labels normalize before storage and must exist before assignment.

## Status and priority

Statuses describe lifecycle:

| Status | Meaning |
| --- | --- |
| `inbox` | Captured work that needs triage |
| `backlog` | Work for later |
| `todo` | Ready work |
| `active` | Work in progress |
| `done` | Completed work |
| `canceled` | Intentionally dropped work |

Priorities affect queue placement and sorting: `none`, `low`, `medium`, `high`, and `urgent`.

## Queue

The queue is the default attention view in the TUI. It answers: what should I look at next?

Tasks are grouped from most urgent to least urgent:

1. **Needs action**: conflicts, urgent tasks, and stale active tasks
2. **Blocked**: tasks with unresolved open dependencies
3. **Focus**: active tasks and high-priority todo tasks
4. **Triage**: inbox tasks and medium-priority todo tasks
5. **Later**: backlog tasks, low-priority todo tasks, and todo tasks with no priority

Inside a group, tasks use a deterministic local score. More important statuses come first, higher priorities come first, stale tasks move upward, and older tasks win ties.

## Refs

aven prints display refs such as `APP-7KQ9`. The full task id is the stable identity. The printed suffix is shortened for display and can lengthen to stay unique inside a workspace. The project prefix is display context.

Prefer the qualified ref printed by command output. Bare suffix refs work when they are unambiguous. Typed suffix refs must contain at least three characters.

## Descriptions and notes

Use descriptions for the main body of a task: problem statement, acceptance criteria, relevant links, and implementation details.

Use notes for durable history and handoff context: decisions, blockers, partial progress, and follow-up details. Notes are append-style entries, appear in full task reads, and can be deleted when needed.

## Epics and dependencies

Epics group related work. An epic is a task that contains child tasks. Epic membership does not make a child blocked.

Epic children belong to the same workspace and project as the epic. Each child belongs to one epic, and a child cannot itself be an epic.

Dependencies model ordering. If task A depends on task B, task B must finish before task A can start.

Dependencies cannot point to the same task, cross workspaces, or form cycles. Unresolved open dependencies drive blocked and ready behavior.

Use epics for grouping. Use dependencies for ordering.

## Deleted tasks and conflicts

Deleted tasks are soft-deleted. They stay available through explicit deleted-task list and search options.

Sync conflicts are explicit and field based. Conflicted tasks appear in the TUI and can be inspected before you choose the final value.
