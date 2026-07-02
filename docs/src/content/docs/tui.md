---
title: TUI
description: Use the keyboard-first terminal interface.
---

Open the TUI with:

```sh
aven tui
```

The TUI is the main human interface for aven. It uses the same local database and workspace routing as the rest of the tool.

## Find work

The TUI opens to the queue. The queue groups work into needs action, blocked, focus, triage, and later.

Use the sidebar to switch scope and views. Views include queue, open, inbox, backlog, todo, active, done, and conflicts.

Project selection is scope. Select a project to focus the task list on that project.

## Add and edit tasks

Press `a` to add a task. The composer can capture title, description, project, status, and priority. Natural-language task intake can create a task through the configured agent command.

Use task actions to edit status, priority, project, labels, descriptions, and notes. Press `n` to add a note, `s` to open the status picker, `d` to mark done, and `x` to mark canceled.

`Ctrl-x Ctrl-e` opens an external editor from text entry flows that support it.

Press `u` to undo a completed TUI mutation.

## Open detail

Press `Enter` on a task to open its detail view. Double-clicking a task row also opens detail when mouse support is active.

In detail view, use `j/k`, arrows, `Ctrl-d`, `Ctrl-u`, `PageDown`, `PageUp`, or the mouse wheel to scroll. Use `[` and `]` to switch tasks while staying in detail. Press `Esc`, `Enter`, or `q` to return to the list.

Clicking status or priority in detail opens the matching menu and returns to detail after selection.

## Search and filter

Press `/` to search. Search finds tasks by title, description, project, label, note, status, priority, or ref.

Search shows live preview results. Use `Ctrl-n` and `Ctrl-p` to move through results. `Enter` opens the selected task detail. `Tab` accepts the query and opens the search-results view.

Filters refine the current scope and view. Common filters include label, priority, and deleted visibility.

Ordering changes the sort field or direction. Queue ordering uses aven's attention score, while other views can use supported sort fields.

## Mouse support

The TUI supports mouse actions in addition to keyboard shortcuts:

- Click header menus for workspace, scope, view, ordering, and sync status.
- Click header metrics to jump to related views.
- Right-click a task status cell to open the status menu.
- Double-click a task row to open detail.
- Scroll detail content with the mouse wheel.

## Conflicts, epics, and dependencies

Use the conflicts view to inspect conflicted tasks. Conflict actions live under the `c` prefix family.

Task detail shows why a task is blocked and what it unlocks. Task actions include dependency and epic workflows when those relationships are useful.

## Shortcuts

Direct shortcuts cover common movement and actions:

| Shortcut | Action |
| --- | --- |
| `j`, `k`, arrows | Move selection |
| `Enter` | Open selected task detail |
| `/` | Search |
| `:` | Open command palette |
| `?` | Open help |
| `r` | Refresh |
| `Tab` | Switch focus |
| `u` | Undo |
| `q` | Quit |

Fast aliases include `a` for add task, `n` for add note, `s` for status picker, `d` for done, and `x` for canceled.

## Prefix families

Intent shortcuts use mnemonic prefixes. Press a prefix to see hints, then press the next key.

| Prefix | Family |
| --- | --- |
| `g` | Go to task list scope or switch workspace |
| `v` | Views |
| `f` | Filters |
| `o` | Ordering |
| `t` | Task fields and lifecycle actions |
| `p` | Project administration |
| `L` | Label administration |
| `c` | Conflicts |
| `C` | Config |

The help and command palette show the active command catalog. Some catalog entries can be disabled when the feature is unavailable.

`Esc` cancels prefix mode and overlays. The command palette searches the same catalog as prefix hints and help.
