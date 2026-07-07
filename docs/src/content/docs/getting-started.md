---
title: Getting started
description: Install aven and start using the TUI.
---

## Install

Use the install script:

```sh
curl -fsSL https://raw.githubusercontent.com/raine/aven/main/scripts/install | bash
```

Or install with Homebrew:

```sh
brew install raine/aven/aven
```

## Open the TUI

```sh
aven tui
```

Tasks live in a local SQLite database.

The TUI is the human interface for aven. It opens to the queue, which highlights tasks that need attention.

Use [Configuration](/configuration/) later when you want a specific database path, sync server, workspace routes, or project path mappings. Use `aven doctor` when config or database routing is unclear.

## First session

1. Press `a` to add a task.
2. Fill in the title. Add description, project, status, or priority when useful.
3. Press `Enter` on a task to open its detail view.
4. Use `s` to change status, `d` to mark done, and `u` to undo a TUI change.
5. Press `?` for help or `:` for the command palette.

## Capture from tmux

When you live in tmux, open the add task composer as a popup over the current pane:

```sh
aven tmux add-task-popup
```

![aven tmux popup add task composer over terminal panes](/tmux-popup.webp)

Use `--print-binding` to print a tmux key binding command for your tmux config.

## Next steps

- Read [Concepts](/concepts/) to learn the task model.
- Read [Configuration](/configuration/) when you want workspace routes, project path mappings, sync defaults, or a specific database path.
- Read [TUI](/tui/) for navigation, views, filters, and shortcuts.
- Read [Agents](/agents/) for CLI and coding-agent workflows.
