---
title: Workflows
description: Use aven from daily capture, terminal, chat, and agent workflows.
---

aven is most useful when work can enter the task store from the place where it appears. The TUI is the main human interface, the CLI is the automation surface, and synced setups can add more entrypoints without tying tasks to a repository checkout.

## Capture from the TUI

The TUI is the fastest way to capture, triage, and inspect tasks when you are at your terminal:

```sh
aven tui
```

Press `a` to open the task composer. Use [TUI](/tui/) for the full keyboard workflow, including LLM task intake, tmux popup capture, detail view, search, filters, and command palette.

## LLM task intake

LLM task intake is documented in [TUI capture tasks](/tui/#capture-tasks). In the composer, `Ctrl-n` turns rough input, pasted notes, or dictated rambling into a sensible task title and description while preserving the useful context.

The LLM command itself is configured through `agent.task_intake`; see [Configuration](/configuration/#agent-task-intake).

## Capture from chat

A synced aven setup can turn chat or voice input into project-scoped tasks without a repo checkout.

For example, a Raspberry Pi can run a private Telegram agent with voice transcription. The agent uses aven guidance to map spoken project names to aven projects, then calls a small `aven-add` wrapper that syncs, creates the task, captures the ref, and syncs again.

An idea captured on the go is already in the right project when you get back to your laptop.

## Work with coding agents

Coding agents can use aven tasks as durable work items. The usual flow is:

1. Capture or triage work in aven.
2. Start an agent from the repository directory.
3. Let `aven prime` load current task context.
4. Ask the agent to work on a specific ref, or to choose ready work.
5. Review the code change and the task note the agent leaves behind.

Use [Agents](/agents/) for skill installation, automatic priming, agent prompts, and handoff notes.

## Keep tasks synced

Sync keeps tasks available across laptops, agents, servers, and chat entrypoints. Each client writes locally first, then pushes and pulls changes through a self-hosted server.

Use [Sync and backups](/sync/) for server setup, daemon sync, backups, and conflict handling.
