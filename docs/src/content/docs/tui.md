---
title: Using the TUI
description: Find, capture, and manage work in the terminal interface.
---

The TUI is aven's keyboard-driven interface for managing tasks locally.

Some icons require a Nerd Font selected in your terminal profile. If you see
blanks or boxes, follow the [terminal font setup](/getting-started/#terminal-font).

```sh
aven
```

Use it to find work, capture tasks, edit them, and inspect their details. See
[Organizing tasks](/organize-tasks/) for task structure, [Schedule
tasks](/schedule-tasks/) for availability and deadlines, and [Recurring
tasks](/recurring-tasks/) for repeating work.

![aven TUI showing the queue view across workspace projects](/tui.webp)

<p class="media-caption">Queue groups open work across the active workspace. The preview below the list shows details for the selected task.</p>

Use `?` for commands available in the current mode. Use `:` to search the complete command catalog by name or description. [Custom TUI commands](/custom-commands/) can add trusted local programs to this catalog.

![aven TUI command palette filtering view commands](/command-palette.webp)

<p class="media-caption">The command palette searches the catalog used by contextual help and prefix hints.</p>

## Find work

Queue is the default view. It ranks available work by what needs attention across the active scope, including tasks with unresolved blockers. Ready is the subset that can be started: it excludes epics, deferred tasks, and tasks with unresolved dependencies. See [Concepts](/concepts/) for queue groups, statuses, priorities, availability, due dates, dependencies, epics, and related tasks.

Use the sidebar or the `v` command family to switch queries and layouts.

| Goal | Query or layout | Open with |
| --- | --- | --- |
| Decide what needs attention | Queue | `v q` |
| Start work without an unresolved blocker | Ready | `v y` |
| Review unresolved dependencies | Blocked | `v k` |
| Review deadlines before today | Overdue | `v x` |
| Toggle a compatible query between list and lifecycle lanes | List or columns layout | `v l` |
| See all unfinished work | Open | `v o` |
| Triage captured work | Inbox | `v i` |
| Review work saved for later | Backlog | `v b` |
| See committed work | Todo | `v t` |
| See work in progress | Active | `v a` |
| Review deferred work by availability | Upcoming | Sidebar or `aven tui --view upcoming` |
| Review completed and canceled work | Done | `v d` |
| Resolve synchronization conflicts | Conflicts | `v c` |
| Plan with parent tasks | Epics | `v e` |
| Manage repeating schedules | Recurring Tasks | `v u` |
| Audit recent changes | Recent actions | `v r` |
| Return to accepted search results | Search results | `v s` |

### Scope

Workspace scope shows tasks across projects. Project scope narrows every view to one project.

| Key | Action |
| --- | --- |
| `g a` | Show all projects in the workspace |
| `g p` | Choose a project scope |
| `g w` | Switch workspace |

Type in the project and workspace pickers to filter them.

The header and sidebar show the active workspace, scope, view, and filters.
Click a sidebar heading, or focus it and press `Enter`, to collapse or expand
that section. Choose which views the sidebar lists with
[`tui.sidebar.views`](/configuration/#tui-sidebar-views).

Use `Tab` (or `Left` from the task list) to focus the sidebar. Browse with
`Up`/`Down` or `k`/`j`, page with `PageUp`/`PageDown` or `Ctrl-u`/`Ctrl-d`, and
use `Home`/`End` to reach the edges. Long lists keep the highlighted row visible;
page navigation and mouse wheel movement stop at the first and last selectable
rows. While browsing tasks, the wheel over the visible sidebar focuses and
browses it without changing the active scope or task selection. Active task
detail and capturing overlays retain wheel ownership. Press `Enter` to activate
the highlighted view or project, or `Esc`/`Tab` to return to tasks without
activating it.

### Search, filter, and order

Press `/` to search task text, notes, labels, metadata, and refs as you type. `Enter` opens the selected result, and `Tab` opens the full Search results view.

Filters constrain the current view. Useful filter commands include:

| Key | Action |
| --- | --- |
| `f l` | Filter by label |
| `f p` | Filter by priority |
| `f r` | Cycle recurring lifecycle |
| `f x` | Cycle deleted-task visibility |
| `f c` | Clear filters |

Press `o` to choose ordering. Queue uses aven's attention score. Other views can order by fields such as availability and due date.

For shell shortcuts that open an initial view, project, filter, or task, see [`aven tui`](/command-reference/#aven-tui).

## Capture tasks

Press `a` to open the task composer. Project, status, priority, labels, availability, due date, natural-language schedule, title, and description stay visible as one form. The active field has a `▶` marker, so focus remains clear without relying on color. The project picker's **Infer** option uses the project mapped to the current directory. Choose a named project when the task belongs elsewhere.

![aven TUI add task popup with title and description fields](/add-task.webp)

<p class="media-caption">Set task metadata, scheduling, title, and description in the same form.</p>

Press `Ctrl-s` to create the task from any field, or `Ctrl-Enter` in terminals that support it. `Enter` creates from the title, opens pickers on metadata fields, and adds a newline in the description. Text fields support `Alt-Left` and `Alt-Right` to move by word and `Alt-Backspace` to delete one.

Press `Ctrl-g` to create the task and start another with the same project, status, priority, and labels. It isn't available for repeating tasks.

New tasks default to `inbox`, or `todo` when they have a priority or repeat. The status field marks this default with `(auto)`. Choosing a status overrides it; choose **Auto** to go back.

### Set a schedule

The **Schedule** field accepts natural expressions such as `tomorrow`,
`due next Friday`, or `every Friday at 09:00`. Press `Enter` on the field for a
structured editor with **One-off** and **Repeating** modes.

See [Scheduling tasks](/schedule-tasks/) for availability and deadlines, and
[Recurring tasks](/recurring-tasks/) for repeating schedules.

### Create with AI

Press `Ctrl-n` when the visible title and description contain rough notes, pasted context, or dictated input. The configured task-intake agent produces a structured draft for review. See [`agent.task_intake`](/configuration/#agent-task-intake). If you close the TUI before the task is created, Aven sends a [notification](/notifications/) when it is.

### Attach images

Copy a PNG, JPEG, GIF, or WebP image and press `Ctrl-v`, or paste a local image path or `file://` URL. Images remain attached to the draft when validation fails.

If clipboard image paste is unavailable, save the image and paste its path. See [terminal tips](/tips/) for terminal-specific preview setup, [Configuration](/configuration/#png-optimization) for image optimization, and [`aven attachment`](/command-reference/#aven-attachment) for limits and command-line management.

## Triage and edit tasks

Direct shortcuts cover frequent lifecycle changes:

| Key | Action |
| --- | --- |
| `s` | Choose status |
| `!` | Choose priority |
| `d` | Mark done |
| `x` | Mark canceled |
| `n` | Add a note |
| `u` | Undo the latest completed TUI mutation |

Use the `e` family to edit task fields:

| Key | Field |
| --- | --- |
| `e t` | Title |
| `e d` | Description |
| `e j` | Project |
| `e p` | Priority |
| `e a` | Availability |
| `e u` | Due date |
| `e l` | Labels |

The `t` family contains lifecycle, priority, relationship, recurrence, delete, and restore actions. Press `t` and follow the footer hints, or search by action name with `:`.

### Delete and restore

`t D` deletes the selected task. Show deleted tasks with `f x`, then press `t R` to restore one. Use `x` instead to keep a task as canceled.

### Mark and change several tasks

| Key | Action |
| --- | --- |
| `Space` | Toggle the selected task's mark |
| `t V` | Toggle marks on visible tasks |
| `t C` | Clear all marks |

Most edits, deletes, and column moves then apply to every marked task, and `u` undoes them together.

## Columns

Columns shows the current view as lifecycle lanes.

![aven TUI showing tasks organized across lifecycle columns](/columns.webp)

<p class="media-caption">Columns arranges tasks into lifecycle lanes and shows details for the selected task.</p>

Press `v l` to switch between list and columns. Views with their own layout, such as Queue and Upcoming, show `list only` in the header. Arrow keys move within and between lanes. Press `<` or `>` to move the selected or marked tasks one lane, or `m` to choose a destination. A moved task takes the lane's first status.

Press `g d` to toggle the selected-task preview. Configure lane names and status groups under [`tui.columns`](/configuration/#tui-columns).

## Task detail

Press `Enter` on a task to open detail.

![aven TUI task detail view with Markdown description and task metadata](/task-detail.webp)

<p class="media-caption">Task detail shows Markdown descriptions, notes, relationships, attachments, and editable metadata.</p>

Use `[` and `]` to switch tasks without returning to the list.

### Copy task information

The `y` family copies task information from either the list or detail:

| Key | Copies |
| --- | --- |
| `y r` | Display ref, such as `APP-7KQ9` |
| `y i` | Durable task ID |
| `y t` | Title |
| `y d` | Description |
| `y a` | Title and description |
| `y n` | Notes |
| `y m` | Complete task report as Markdown |

With tasks marked, `y r`, `y i`, and `y t` copy one line per marked task.

In detail, drag across rendered title or description text and press `y` to copy only that selection.

<div class="video-player" data-video-player>
  <video controls muted playsinline preload="metadata">
    <source src="/task-detail-text-selection.mp4" type="video/mp4" />
  </video>
  <button type="button" class="video-player-toggle" aria-label="Play video">
    <span class="video-player-toggle-icon" aria-hidden="true"></span>
  </button>
</div>

<p class="media-caption">Drag across rendered task text and press <code>y</code> to copy the selection.</p>

### Publish a task report as a GitHub gist

From task detail, press `t g` to publish the Markdown task report as a secret GitHub gist and copy its URL. Secret gists are unlisted, not private: anyone with the URL can view them. Requires an authenticated `gh` CLI. See an [example task report](https://gist.github.com/raine/52e230cc05e4dc205e5e1394aa1b32a3).

### Image attachments

Attachments appear below the description. iTerm2, Kitty, WezTerm, and Ghostty can show inline previews. Other terminals show a text label.

![Aven task detail showing two attached Wayfinder design concepts with the first inline preview focused](/task-attachments.webp)

<p class="media-caption">Kitty showing an inline image attachment preview in task detail.</p>

Open the preview to move between attachments, open one in the system viewer, or delete one. To save an image as a regular file, use [`aven attachment get`](/command-reference/#aven-attachment).

## Projects and relationships

Press `p` to manage projects and `L` to manage labels. `t B` and `t U` add and
remove blockers, and `g B` jumps to a blocker. Epic actions live under `t c`. In
task detail, focus a relationship to open or unlink its task.

See [Organizing tasks](/organize-tasks/) for choosing among workspaces, projects,
labels, epics, dependencies, and related tasks.

## Recurring tasks

Press `v u` to open Recurring Tasks. Each row shows its schedule, next date, and
state. Press `Enter` on a recurring task to open its detail; the footer shows the
actions available for that state. The `t r` family also works from a selected
recurring task or one of its dated tasks.

See [Recurring tasks](/recurring-tasks/) for schedules, lifecycle actions, and
history. See [`aven recur`](/command-reference/#aven-recur) for CLI management.

## Conflicts and sync

The header shows synchronization state. Click it, press `C s`, or run `:sync` to open the **Sync** dialog, where you can set up or join sync, sync now, add a device, and manage devices. Press `S` to sync immediately. Press `v c` to open tasks with unresolved field conflicts, then use the `c` family to inspect and resolve them.

See [Sync across devices](/sync/setup/) for setup, transport, and conflict handling.
See [Back up and restore](/backups/) for recovery workflows.

## Run aven from tmux

Bind the project-scoped TUI, workspace-wide TUI, composer, or any combination to
tmux popups:

```text
bind C-w display-popup -E -d '#{pane_current_path}' -w 80% -h 80% 'aven tui -p'
bind C-a display-popup -E -d '#{pane_current_path}' -w 80% -h 80% 'aven'
bind t display-popup -E -d '#{pane_current_path}' -w 120 -h 30 'aven tui --add-task-only'
```

The `-p` flag scopes Aven to the project inferred from the active pane's current
directory. Omit it to open Aven at workspace scope.

![aven TUI running in a tmux popup over terminal panes](/tui-tmux-popup.webp)

<p class="media-caption">The full TUI opens over the current tmux pane.</p>

![aven task composer running in a tmux popup](/tmux-popup.webp)

<p class="media-caption">A smaller tmux popup opens directly into the task composer.</p>

See [terminal tips](/tips/#use-ctrl-enter-in-alacritty-and-tmux) for modified-key and tmux configuration.

## Mouse support

Mouse actions cover the same common outcomes as keyboard commands:

- Click the sidebar or header to change query, layout, scope, project, filter, or ordering.
- Double-click a task to open detail.
- In the columns layout, click a lane header to move selected or marked tasks, or right-click a task to choose status.
- Click an inline image to open its TUI preview, or its text label to use the system viewer.
- Scroll task detail with the mouse wheel.

## Discover commands

The in-app command catalog is the authoritative shortcut reference:

- `?` lists commands available in the current mode.
- `:` searches command names and descriptions. `Tab` cycles through matches.
- `:theme` switches between `dark`, `light`, and `terminal` colors for this
  session without changing your config.
- Prefix keys show their available continuations in the footer.

| Prefix | Family |
| --- | --- |
| `g` | Navigation, scope, and workspace |
| `v` | Views |
| `f` | Filters |
| `o` | Ordering |
| `e` | Edit task fields |
| `t` | Task lifecycle and relationships |
| `y` | Copy task information |
| `p` | Project administration |
| `L` | Label administration |
| `c` | Conflict resolution |
| `C` | Configuration |

## Custom metadata

Task detail shows custom metadata in its own section. Press `e m` to edit a
field: pick one from the list, type a value, and press `Enter` to save. For
multiline values, press `Ctrl-x Ctrl-e` to use your `$VISUAL` or `$EDITOR`.
Define and rename fields with the CLI; see [Task metadata](/task-metadata/).

## Reference

- [Concepts](/concepts/) defines the task model behind the interface.
- [Work with agents](/agents/) connects coding agents and chat integrations to
  Aven.
- [Custom TUI commands](/custom-commands/) documents local programs, JSON task
  context, and command lifecycle settings.
- [Command reference](/command-reference/) documents CLI equivalents and input
  grammar.
- [Configuration](/configuration/) covers workspace routes, project mappings,
  sync defaults, and task-intake configuration.
- [Terminal tips](/tips/) covers terminal input, tmux, and
  image previews.
