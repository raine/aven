---
title: Why not Taskwarrior?
description: How aven differs from Taskwarrior.
---

Taskwarrior is a great task manager and a major inspiration for aven. aven exists because I wanted a similar power-user, local-first tool with different defaults for coding-agent workflows, task identity, context, workspaces, and the human interface.

## Different goals

Taskwarrior is built around a powerful CLI. aven is designed so the CLI is agents-first, and the TUI is for humans.

For humans, the TUI is optimized for speed. It starts instantly, supports keyboard-first workflows, and makes common task operations faster than running command sequences.

For agents, the CLI output is compact and explicit. Commands print the task refs and fields agents need to update status, add notes, and leave handoff context without scraping a human UI.

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

Workspace routes can select a workspace based on the current directory, so working under `~/work` can use the work workspace automatically.

## Repo-independent storage

Tasks live outside repositories. Repos provide project context, but they do not own the task data.

That means tasks survive branches, worktrees, checkouts, and clones. It also leaves room for synced entrypoints such as an iOS app or Telegram agent to manage tasks without cloning every repository.

## Why build a separate tool?

aven owns the full stack: data model, CLI, TUI, sync, and agent workflow. That makes these choices part of the product instead of layers on top of another task manager.
