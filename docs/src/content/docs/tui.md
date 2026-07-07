---
title: TUI
description: Use the keyboard-first terminal interface.
---

The TUI is the main human interface for aven. It gives you a fast, keyboard-driven view of your local task database, with the same workspace routing and project inference used by the CLI.

```sh
aven tui
```

![aven TUI showing the queue view across workspace projects](/tui.webp)

<p style="color: var(--sl-color-gray-3); font-size: 0.875rem; margin-top: -0.75rem;">The queue view brings workspace scope, project groups, task metadata, and selected-task context into one screen.</p>

## Screen tour

The TUI is organized around the current workspace, scope, view, task list, and selected task.

| Area | Purpose |
| --- | --- |
| Header | Shows the active workspace, scope, view, queue counts, and sync state. |
| Sidebar | Switches between views, workspace scope, projects, and filters. |
| Task list | Shows the tasks in the current view, including refs, labels, project, status, priority, and age. |
| Selected task | Shows the selected task's important metadata and description without leaving the list. |
| Footer | Shows the most useful shortcuts for the current mode. |
| Overlays | Add tasks, search, command palette, help, and pickers appear over the current screen. |

## Interaction model

aven is keyboard-first. Common actions have direct shortcuts, broader command families use prefixes, and the command palette searches the same command catalog.

- **Direct shortcuts** run frequent actions immediately. For example, `a` adds a task, `s` opens the status picker, `d` marks a task done, and `u` undoes a completed TUI mutation.
- **Prefix families** group related commands. Press `v` for views, `f` for filters, `o` for ordering, `t` for task actions, `p` for projects, `L` for labels, `c` for conflicts, or `C` for config.
- **Command palette** opens with `:` and searches available commands by name and description.
- **Help** opens with `?` and shows commands available in the current mode.
- **Escape** cancels overlays and prefix mode.

![aven TUI command palette filtering view commands](/command-palette.webp)

<p style="color: var(--sl-color-gray-3); font-size: 0.875rem; margin-top: -0.75rem;">The command palette searches the same command catalog used by prefix hints and help.</p>

## Daily workflow

A typical TUI session is simple:

1. Open `aven tui`.
2. Start in the queue to see work that needs attention.
3. Press `a` to capture new work.
4. Use `s`, `d`, `x`, and task actions to triage the selected task.
5. Press `Enter` to inspect details, notes, dependencies, and metadata.
6. Use `/`, `f`, `o`, `v`, and `g` when the list needs narrowing or a different view.
7. Press `u` when a completed TUI change should be undone.

## Find work

The queue is the default attention view. It groups work into needs action, blocked, focus, triage, and later. The queue gives you one place to decide what needs attention across projects.

Use the sidebar or the `v` prefix family to switch views. Views include queue, open, inbox, backlog, todo, active, done, conflicts, epics, recent actions, and search results.

Use the sidebar or the `g` prefix family to switch scope. Workspace scope shows projects in the active workspace. Project scope narrows the task list to one project.

Queue behavior, statuses, priorities, refs, dependencies, and epics are described in [Concepts](/concepts/).

## Capture tasks

Press `a` to open the task composer. The composer captures title, description, project, status, and priority.

![aven TUI add task popup with title and description fields](/add-task.webp)

<p style="color: var(--sl-color-gray-3); font-size: 0.875rem; margin-top: -0.75rem;">The composer captures structured fields without leaving the keyboard workflow.</p>

The composer also supports LLM task intake. Use `Ctrl-n` when you have rough input, pasted notes, or dictated rambling. The configured LLM turns that input into a sensible task title and description while preserving the useful context.

Use `Tab` to move into the description field. Use `Ctrl-t` for status, `Ctrl-p` for project, and `Ctrl-r` for priority. Press `Enter` to create the task, or `Esc` to cancel.

`Ctrl-x Ctrl-e` opens an external editor from text entry flows that support it.

## Triage and edit tasks

Use direct shortcuts for common task changes:

- `s` opens the status picker.
- `d` marks the selected task done.
- `x` marks the selected task canceled.
- `n` adds a note.
- `u` undoes a completed TUI mutation.

Use the `t` prefix family for more task fields and lifecycle actions. Task actions include editing status, priority, project, labels, descriptions, notes, dependencies, and epic relationships.

## Open detail

Press `Enter` on a task to open its detail view. Double-clicking a task row also opens detail when mouse support is active.

![aven TUI task detail view with Markdown description and task metadata](/task-detail.webp)

<p style="color: var(--sl-color-gray-3); font-size: 0.875rem; margin-top: -0.75rem;">The detail view keeps Markdown context, notes, relationships, and editable metadata together.</p>

The detail view renders Markdown descriptions, notes, and task metadata. Use `j/k`, arrows, `Ctrl-d`, `Ctrl-u`, `PageDown`, `PageUp`, or the mouse wheel to scroll. Use `[` and `]` to switch tasks while staying in detail. Press `Esc`, `Enter`, or `q` to return to the list.

Clicking status or priority in detail opens the matching menu and returns to detail after selection.

## Search, filter, and order

Search, filters, and ordering are separate tools:

- **Search** finds tasks by title, description, project, label, note, status, priority, or ref.
- **Filters** constrain the current task list by fields such as label, priority, and deleted visibility.
- **Ordering** changes the sort field or direction. Queue ordering uses aven's attention score, while other views can use supported sort fields.

Press `/` to search. Search shows live preview results while you type.

| Key                | Search behavior                                    |
| ------------------ | -------------------------------------------------- |
| `Ctrl-n`, `Ctrl-p` | Move through preview results.                      |
| `Enter`            | Open the selected preview result.                  |
| `Tab`              | Accept the query and open the search-results view. |

Press `f` to filter the current list. Press `o` to change ordering.

## Projects, labels, dependencies, and epics

Projects and labels are available from the sidebar and command families. Use `p` for project administration and `L` for label administration.

Task detail shows why a task is blocked and what it unlocks. Task actions include dependency and epic workflows when those relationships are useful.

## Conflicts and sync state

The header shows sync state, and the conflicts view shows tasks that need human review. Use the conflicts view to inspect conflicted tasks. Conflict actions live under the `c` prefix family.

Sync setup and conflict concepts are described in [Sync and backups](/sync/).

## Mouse support

The TUI supports mouse actions in addition to keyboard shortcuts:

- Click header menus for workspace, scope, view, ordering, and sync status.
- Click header metrics to jump to related views.
- Click sidebar entries to switch views, scope, projects, and filters.
- Right-click a task status cell to open the status menu.
- Double-click a task row to open detail.
- Scroll detail content with the mouse wheel.

## Keyboard reference

### Global navigation

| Shortcut         | Action                        |
| ---------------- | ----------------------------- |
| `j`, `k`, arrows | Move selection                |
| `Tab`            | Switch focus                  |
| `Enter`          | Open selected task detail     |
| `[`, `]`         | Switch tasks while in detail  |
| `/`              | Open search                   |
| `:`              | Open command palette          |
| `?`              | Open help                     |
| `r`              | Refresh                       |
| `u`              | Undo                          |
| `q`              | Quit                          |
| `Esc`            | Cancel overlay or prefix mode |

### Task shortcuts

| Shortcut        | Action                                           |
| --------------- | ------------------------------------------------ |
| `a`             | Add task                                         |
| `n`             | Add note                                         |
| `s`             | Open status picker                               |
| `d`             | Mark done                                        |
| `x`             | Mark canceled                                    |
| `Ctrl-x Ctrl-e` | Open external editor during supported text input |

### Prefix families

Press the prefix to see available commands in that family.

| Prefix | Family                                    |
| ------ | ----------------------------------------- |
| `g`    | Go to task list scope or switch workspace |
| `v`    | Views                                     |
| `f`    | Filters                                   |
| `o`    | Ordering                                  |
| `t`    | Task fields and lifecycle actions         |
| `p`    | Project administration                    |
| `L`    | Label administration                      |
| `c`    | Conflicts                                 |
| `C`    | Config                                    |

## Related pages

- [Getting started](/getting-started/) covers installation and first-run usage.
- [Concepts](/concepts/) explains the task model behind the TUI.
- [Configuration](/configuration/) covers workspace routes, project path mappings, sync defaults, and LLM task intake configuration.
- [Agents](/agents/) covers CLI and coding-agent workflows.
