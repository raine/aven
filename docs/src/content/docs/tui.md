---
title: TUI
description: Use the keyboard-first terminal interface.
---

The TUI is the main human interface for aven. It gives you a fast, keyboard-driven view of your local task database, with the same workspace routing and project inference used by the CLI.

```sh
aven tui
```

![aven TUI showing the queue view across workspace projects](/tui.webp)

<p class="media-caption">The queue view brings workspace scope, project groups, task metadata, and selected-task context into one screen.</p>

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

<p class="media-caption">The command palette searches the same command catalog used by prefix hints and help.</p>

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

Use the sidebar or the `v` prefix family to switch views. Views include queue, columns, open, inbox, backlog, todo, active, done, conflicts, epics, recent actions, and search results.

### Columns view

The Columns view arranges tasks into configurable lifecycle lanes. Its defaults use Aven's status names directly: Inbox, Backlog, Todo, Active, and Done. Inbox shows the oldest captures first, Backlog and Todo prioritize important older work, Active surfaces stale work, and Done shows the most recently completed or canceled tasks first.

![aven TUI showing tasks organized across lifecycle columns](/columns.webp)

<p class="media-caption">The columns view keeps the complete task lifecycle visible while preserving task metadata and selected-task context.</p>

Use `v l` to open the view. Up and down move within a lane, while left and right move the selection between lanes. Press `<` or `>` to move the selected task one lane, or press `m` to choose a destination lane. Marked tasks move together and create one undo step. Relative moves use each task's current lane, and the batch remains unchanged when any marked task is already at the requested edge.

Moving into a lane assigns its first configured status. Choosing the lane a task already occupies preserves its exact status, so a canceled task remains canceled when Done contains `[done, canceled]`. Click a lane header to move the selected or marked tasks there, or right-click a card to open the status choices.

Press `g d` to toggle the selected-task preview and give the board more vertical space. Project scope, filters, details, editing, and task mutations continue to use the existing task model. Configure lane names and status grouping under [`tui.columns`](/configuration/#tui-columns).

Use the sidebar or the `g` prefix family to switch scope. Workspace scope shows projects in the active workspace. Project scope narrows the task list to one project.

Queue behavior, statuses, priorities, refs, dependencies, and epics are described in [Concepts](/concepts/).

## Run the TUI in tmux

When you live in tmux, bind the full TUI to a popup so you can open aven over the current pane without changing windows.

![aven TUI running in a tmux popup over terminal panes](/tui-tmux-popup.webp)

<p class="media-caption">A tmux popup keeps the full queue and detail workflow available over the terminal session you are already using.</p>

For example, this binds prefix + `Ctrl-a` to a large popup in the current pane's directory:

```text
bind C-a display-popup -E -d '#{pane_current_path}' -w 80% -h 80% 'aven tui'
```

## Capture tasks

Press `a` to open the task composer. Project, status, priority, labels, title, and description stay visible as one form. The active field has a `▶` marker, so focus remains clear without relying on color.

![aven TUI add task popup with title and description fields](/add-task.webp)

<p class="media-caption">The composer captures structured fields without leaving the keyboard workflow.</p>

Use `Tab` and `Shift-Tab` to move through every field. Press `Enter` to edit the focused metadata field. `Enter` creates from the title field and inserts a newline in the description. `Ctrl-Enter` creates from any field when the terminal reports modified Enter keys. `Ctrl-s` is the portable create fallback. Press `F1` for complete composer help.

:::note[Terminal compatibility]
Aven negotiates progressive keyboard enhancement with compatible terminals and uses the xterm modified-key protocol as a fallback. In tmux, enable forwarding with `set -s extended-keys on`. The outer terminal must also support tmux's extended-key mode. Use `Ctrl-s` when that path does not distinguish `Ctrl-Enter` from `Enter`. See [Tips](/tips/#use-ctrl-enter-in-alacritty-and-tmux) for the complete Alacritty and tmux configuration.
:::

`Esc` closes an empty composer. A draft with entered or changed values asks for confirmation before it is discarded. Opening metadata controls and help preserves the title and description cursors and viewport.

`Ctrl-x Ctrl-e` opens an external editor while the description is focused. Optional accelerators such as `Ctrl-p`, `Ctrl-t`, `Ctrl-r`, and `Ctrl-l` remain available, but all metadata is accessible with Tab and Enter.

### LLM task intake

Use `Ctrl-n` in the composer to **Create with AI** when you have rough input, pasted notes, or dictated rambling. The configured LLM turns the visible title and description into a sensible task while preserving useful context and the selected project.

The LLM command itself is configured through `agent.task_intake`; see [Configuration](/configuration/#agent-task-intake).

### Capture from tmux

When you live in tmux, bind the task composer to a key so it opens as a popup over the current pane.

![aven tmux popup add task composer over terminal panes](/tmux-popup.webp)

<p class="media-caption">The tmux popup opens the same composer over your current terminal session.</p>

For example, this binds prefix + `t` to a 120 by 30 popup in the current pane's directory:

```text
bind t display-popup -E -d '#{pane_current_path}' -w 120 -h 30 'aven tui --add-task-only'
```

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

<p class="media-caption">The detail view keeps Markdown context, notes, relationships, and editable metadata together.</p>

The detail view renders Markdown descriptions, notes, and task metadata. Use `j/k`, arrows, `Ctrl-d`, `Ctrl-u`, `PageDown`, `PageUp`, or the mouse wheel to scroll. Use `[` and `]` to switch tasks while staying in detail. Press `Esc`, `Enter`, or `q` to return to the list. Clicking status or priority opens the matching menu and returns to detail after selection.

### Select and copy text

Drag across the rendered title or description to select text, then press `y` to copy it. Selection follows the rendered Markdown and remains anchored while the detail view scrolls. Press `Esc` or click outside the selectable text to clear it.

<div class="video-player" data-video-player>
  <video controls muted playsinline preload="metadata">
    <source src="/task-detail-text-selection.mp4" type="video/mp4" />
  </video>
  <button type="button" class="video-player-toggle" aria-label="Play video">
    <span class="video-player-toggle-icon" aria-hidden="true"></span>
  </button>
</div>

<p class="media-caption">Drag across rendered task text and press <code>y</code> to copy the selection.</p>

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
- Right-click a task status cell, or any card in Columns view, to open the status menu.
- Click a lane header in Columns view to move the selected or marked tasks into that lane.
- Double-click a task row or card to open detail.
- Scroll detail content with the mouse wheel.

## Keyboard reference

### Global navigation

| Shortcut         | Action                        |
| ---------------- | ----------------------------- |
| `j`, `k`, up/down | Move within the list or column |
| Left/Right        | Move between column lanes     |
| `Tab`             | Switch focus                  |
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
| `<`, `>`        | Move selected or marked tasks between lanes     |
| `m`             | Choose a destination lane in Columns view       |
| `Space`         | Mark or unmark a task for batch actions          |
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
- [Workflows](/workflows/) covers capture, chat, sync, and agent workflows.
- [Agents](/agents/) covers CLI and coding-agent workflows.
