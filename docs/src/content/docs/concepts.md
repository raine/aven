---
title: Concepts
description: Understand the aven task model.
---

aven stores tasks in a local SQLite database. The TUI is the human interface, and the CLI is the automation and agent interface.

Every command runs inside one active workspace. Inside that workspace, projects usually map to repositories or directories, and tasks carry status, priority, labels, descriptions, notes, image attachments, refs, and relationships.

## Workspaces, projects, tasks, and labels

A workspace is a task universe. Use workspaces to isolate personal tasks, work tasks, or any other separate context. Tasks in different workspaces stay separate.

A project groups tasks inside a workspace. Projects commonly map to repositories or directories. By default, aven infers the project from the current repository or directory name.

A task is a unit of work inside a workspace and, usually, a project. It has a title, workflow state, content, identity, and relationships.

Labels are cross-cutting tags such as `bug`, `docs`, or `ux`. Label names are normalized before storage: uppercase letters become lowercase, and punctuation or spaces become dashes. Create a label explicitly, then assign it to tasks.

See [Organizing tasks](/organize-tasks/) for guidance on choosing among workspaces,
projects, labels, epics, and dependencies.

## Status and priority

Statuses describe lifecycle:

| Status     | Meaning                         |
| ---------- | ------------------------------- |
| `inbox`    | Captured work that needs triage |
| `backlog`  | Work for later                  |
| `todo`     | Ready work                      |
| `active`   | Work in progress                |
| `done`     | Completed work                  |
| `canceled` | Intentionally dropped work      |

Open tasks have `inbox`, `backlog`, `todo`, or `active` status. Terminal tasks have `done` or `canceled` status.

Priorities affect queue placement and sorting: `none`, `low`, `medium`, `high`, and `urgent`.

## Descriptions and notes

Use descriptions for the main body of a task: problem statement, acceptance criteria, relevant links, and implementation details.

Notes are append-style entries attached to a task. Use notes for decisions, blockers, partial progress, and follow-up details. Notes appear in full task reads and can be deleted when needed.

## Image attachments

Add screenshots, diagrams, and other images to a task when they help explain the work. Aven supports PNG, JPEG, GIF, and WebP images. Task detail shows attachments in a dedicated section beneath the description.

You can attach images in the TUI while creating or viewing a task. From the command line, create the task first, then use [`aven attachment`](/command-reference/#aven-attachment). Supported terminals show image previews in task detail, while other terminals show a text placeholder. Locally available images can also open in the operating system viewer from either display. See [Image attachments](/tui/#image-attachments) for preview and viewer controls.

[Sync can copy attachment images between devices](/sync/#image-attachments-during-sync). To move all local data yourself, including the image files stored on that device, use a [backup archive](/backups/). JSON export keeps the attachment records but leaves out the image files.

## Refs

aven prints display refs such as `APP-7KQ9` so tasks are easy to mention in commands, notes, and handoffs.

A qualified ref has two parts:

- `APP`: the project prefix. This gives display context.
- `7KQ9`: the task id suffix. aven lengthens the suffix when needed so refs stay unique inside a workspace.

Prefer the qualified ref printed by command output. Bare suffix refs such as `7KQ9` work when they are unambiguous. When you type a suffix ref in a command, it must contain at least three characters.

## Dependencies and epics

Dependencies model ordering. If task A depends on task B, task B must finish before task A can start. Tasks with unresolved open dependencies are blocked.

Dependencies cannot point to the same task, cross workspaces, or form cycles.

Epics group related work. An epic is a task that contains child tasks. Use epics when several tasks belong to one larger effort.

Epic children belong to the same workspace and project as the epic. Each child belongs to one epic. An epic child is a regular task, and blocking behavior comes from dependencies.

Use epics for grouping. Use dependencies for ordering.

## Availability and due dates

Availability and due dates describe independent parts of a task's timeline:

- `available_at` controls when an open task becomes eligible for attention. A future value hides it from normal lists and queue groups. Upcoming shows deferred work explicitly.
- `due_on` is the local-calendar date when completion is expected. It never hides a task or changes its status. Overdue shows open work whose due date is before today.

A task may use either field or both. A future due date does not defer work. A passed due date does not reveal a deferred task. Combine Upcoming and Overdue when you need to inspect deferred work whose deadline passed.

Availability is a timestamp because it can represent a local time of day or an exact UTC instant. Due dates are date-only values because the deadline applies to the local calendar day. Neither field creates reminders, notifications, recurrence, or automatic status changes.

See [Scheduling tasks](/schedule-tasks/) for one-time planning workflows and date
input examples.

## Recurring tasks

Some work comes back: a weekly review, monthly invoices, or a daily journal. A
recurring task keeps the schedule and details to reuse, while Aven gives you one
unfinished task for the relevant date instead of filling your queue with
copies.

Finish that task and the next one returns on its scheduled date. Skip or cancel
it and Aven records that date as skipped before continuing. Leave it unfinished
past its scheduled date and it becomes missed, so recurring work does not pile
up.

Each dated task is a normal task with its own notes, attachments, and edits.
Schedules can be paused while you are away, resumed later, or stopped for good.
See [Recurring tasks](/recurring-tasks/) to create schedules and understand
availability, history, and future tasks.

## Queue

The queue is the default attention view in the TUI. It answers: what should I look at next?

Tasks are grouped from most urgent to least urgent:

1. **Needs action**: sync conflicts, urgent tasks, stale active tasks, and visible, unblocked tasks due today or overdue
2. **Available**: deferred tasks whose availability time arrived since their latest queue activity
3. **Focus**: active tasks and high-priority todo tasks
4. **Soon**: medium-priority todo tasks
5. **Triage**: inbox tasks
6. **Blocked**: tasks with unresolved open dependencies
7. **Later**: remaining open work, including backlog tasks and todo tasks with low or no priority
8. **Epics**: epic containers, separated from actionable child tasks

A stale active task is active work that has gone without updates long enough to need attention. Conflicts, urgent priority, and stale active state place a task in Needs action before dependency grouping. Other blocked work remains in Blocked, including work whose due date or availability time arrived.

Inside a group, queue order is stable and favors important statuses, higher priorities, stale active work, approaching deadlines, and older tasks. Due today and overdue work receive the strongest deadline boost. Deadlines within the next seven days receive a progressively larger boost as they approach. Deadline scoring applies only to visible, non-epic work, so it does not reveal deferred tasks or promote epic containers as actionable tasks.

## Deleted tasks

Deleted tasks are soft-deleted. They are hidden from normal task views and stay available through explicit deleted-task list and search options.

## Sync conflicts

When sync is enabled, the same task can be edited in more than one place before changes sync. aven records conflicts by field instead of overwriting either side.

Conflicted tasks appear in the TUI and can be inspected before you choose the final value.

## Context resolution

aven resolves the active workspace from `--workspace`, the longest matching workspace route, `workspace.default`, the built-in default, or the only existing workspace.

Workspace routes match the current directory before project inference. Configure routes when paths such as `~/work` or `~/personal` should select different task universes.

Project path mappings override project inference when a path should belong to a specific project. Use them when a checkout directory has a short name, a generated name, or a name that differs from the project you want in task refs.

Use `aven doctor` to inspect the active config, database path, workspace, and project for the current directory. See [Configuration](/configuration/) for the full config shape.
