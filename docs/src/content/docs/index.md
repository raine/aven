---
title: What is aven?
description: A local-first task manager for power users and agents.
---

aven is a local-first task manager for power users and agents. It gives you one overview across projects, task capture from wherever work appears, first-class agent workflows, workspace isolation, and a polished terminal UI.

![aven TUI showing the queue view across workspace projects](/tui.webp)

## Why aven?

- **One queue across projects.** aven gives you one place to see what needs attention, while still keeping each repository in its own project.
- **Capture from wherever work appears.** Add tasks from the TUI, CLI, natural-language intake, tmux popup, or agent workflow without switching systems.
- **Built for coding agents.** Agents get token-efficient output, stable refs, and commands for capturing follow-up work, updating status, and preserving handoff context.
- **A polished terminal UI for humans.** Browse, triage, filter, sort, inspect task detail, undo changes, use mouse support, and open the command palette from the TUI.
- **Local-first with optional sync.** Tasks live in SQLite for offline work, with self-hosted sync available when you want the same tasks across laptops, agents, and other devices.
- **Stable refs and task-local context.** Jira-style refs like `APP-7KQ9`, Markdown descriptions, and append-style notes keep work easy to mention and easy to resume.
- **Workspaces for separate worlds.** Personal and work tasks can share the same tool without sharing the same visible queue.

## Design principles

aven makes a few opinionated choices. Taskwarrior is a major inspiration, and aven makes its own choices around agents, refs, Markdown context, workspaces, and the TUI. See [Taskwarrior comparison](/taskwarrior/) for the comparison.

- **The local database is the working copy.** The TUI and CLI work against SQLite directly. Sync is an optional layer for using the same tasks across laptops, servers, and other devices.
- **Tasks live outside repositories.** A task should survive branch switches, worktrees, repo clones, and dirty git states. Repos provide project context, but they do not own the task data. This design keeps room for entrypoints such as an iOS app or Telegram agent to manage tasks through the synced task store without cloning every repo.
- **The CLI is for agents and automation.** Command output is compact, token-efficient, stable, and explicit. Agents can capture follow-up work, update status, add notes, and leave handoff context without guessing hidden UI state.
- **The TUI is optimized for power users.** The Rust terminal UI starts instantly and makes human workflows faster than CLI command sequences. Keyboard shortcuts cover browsing, triage, editing, filtering, sorting, undo, and navigation.
- **Refs are short names for stable identity.** Tasks have offline-safe ids that can be created on different devices without coordination. aven displays the shortest unique ref it can, like git does with commit hashes, so `APP-7KQ9` is nicer to type and paste than a UUID while still referring to one task.
- **The same task store has many entrypoints.** Tasks should be reachable from the laptop TUI, agent CLI, terminal capture flows, sync clients, and integrations that talk to the local or synced task store. Run the sync server on a Raspberry Pi or home server, then sync devices through a VPN or private network. Because tasks live in the task store instead of repo files, a Telegram agent, future iOS app, or other synced entrypoint can create project-scoped tasks without cloning every repository.
- **Context belongs with the task.** Markdown descriptions and append-style notes keep problem statements, decisions, blockers, and partial progress attached to the work.
- **Workspaces isolate worlds.** Personal and work tasks can use the same tool while keeping queues, refs, labels, and projects separate. Workspace routes can make a directory such as `~/work` open the work workspace automatically, and the TUI can open a workspace explicitly when you want to switch context.
