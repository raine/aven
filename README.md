# aven

`aven` is a local-first task manager for power users and agents. It gives you
one overview across projects, task capture from wherever work appears,
first-class agent workflows, workspace isolation, and a polished terminal UI.

It is currently under active development, but already works really well and I
use it as my daily driver.

If you've tried aven, feedback is welcome! Please
[open an issue](https://github.com/raine/aven/issues) to share it.

If you find aven useful, consider sharing it with others who might benefit from
it.

Docs: <https://aven.raine.dev>

![aven TUI showing the queue view across workspace projects](meta/tui.webp)

## Why aven?

- **Offline-first, repo-independent storage.** Tasks live in a local SQLite
  database, not in tracked files inside each repo. Task capture and agent
  updates stay independent from git state, branches, worktrees, and checkouts.

- **Optional self-hosted sync.** Keep the local-first workflow and make the same
  tasks available across laptops, agents, and other devices through a server you
  control.

- **Projects map to repos.** Each repository becomes a project by default,
  created on demand when you add its first task.

- **Agent-first CLI, human-first TUI.** Agents get token-efficient output,
  stable refs, and commands for capturing follow-up work, updating tasks, and
  preserving context from AI coding agents. Humans get a polished,
  keyboard-first TUI with project views, filters, sorting, task detail, undo,
  mouse support, and command palette.

- **Unique task IDs.** Short, Jira/Linear-style IDs like `APP-7KQ9` work
  offline and show each task's project at a glance.

- **Workspaces are first class.** Personal and work tasks can share the same
  tool without sharing the same visible task universe.

- **Markdown-native tasks.** Tasks carry Markdown descriptions and append-style
  notes, so context stays with the task.

- **Fast capture from anywhere.** Natural-language task intake, tmux popup
  capture, scheduling tasks to appear later, and agent-friendly commands make
  it easy to add tasks and surface them when attention becomes useful.

Inspired by Taskwarrior. See
[aven and Taskwarrior](https://aven.raine.dev/taskwarrior/).

## Quick start

Install with the release script:

```sh
curl -fsSL https://raw.githubusercontent.com/raine/aven/main/scripts/install | bash
```

Or install with Homebrew:

```sh
brew install raine/aven/aven
```

Open Aven:

```sh
aven tui
```

Or try the demo:

```sh
aven demo
```

Every demo run starts from the same sample data, and changes are discarded on
exit.

See [Getting started](https://aven.raine.dev/getting-started/) for first-run
usage.

## Documentation

- [What is aven?](https://aven.raine.dev/)
- [Getting started](https://aven.raine.dev/getting-started/)
- [Concepts](https://aven.raine.dev/concepts/)
- [Configuration](https://aven.raine.dev/configuration/)
- [TUI](https://aven.raine.dev/tui/)
- [Agents](https://aven.raine.dev/agents/)
- [Sync and backups](https://aven.raine.dev/sync/)
