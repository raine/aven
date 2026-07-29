---
title: Work with agents
description: Connect coding, chat, and other AI agents to Aven.
---

Aven gives AI agents access to the same local task system you use in the TUI.
Coding agents can load project context and leave durable handoff notes. Chat or
voice agents can capture tasks through the CLI and synchronize them to your
other devices.

## Install the aven skill

```sh
aven skill install
```

`aven skill install` writes the bundled skill to every detected coding-agent skill directory. Detection checks Claude Code, OpenCode, and Codex user directories, plus matching agent config directories in the current workspace.

Use `--agent` to choose explicit targets:

```sh
aven skill install --agent claude
aven skill install --agent opencode
aven skill install --agent codex
```

Explicit targets are installed even when the agent directory is absent. The command reports a clear error when no supported agent is detected and no target is provided.

## Set up automatic priming

Run `aven prime` automatically when an agent session starts. In Claude Code, add it to the `SessionStart` hook in `~/.claude/settings.json`:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "aven prime"
          }
        ]
      }
    ]
  }
}
```

With this hook, every new Claude Code session receives aven instructions and live project task context automatically. Other agent environments can use the same pattern: run `aven prime` at session start and include its output in the agent context.

## Project context

aven infers the active workspace and project from the current directory. Start an agent from a repository directory and automatic priming loads the matching project context.

Use [Configuration](/configuration/) when directory names, workspace routes, or project path mappings need to be explicit. Use `aven doctor` from the same directory to inspect the active database, workspace, project, and routing decisions.

## Work on tasks with coding agents

A typical workflow looks like this:

1. Capture or triage work in aven.
2. Start your agent from the repository directory.
3. Let the startup hook load aven context.
4. Ask the agent to work on a specific ref, or to choose ready work.
5. Review the code change and the task note the agent leaves behind.

Example prompts:

```txt
Work on APP-7KQ9. Use aven for status and handoff notes.
```

```txt
Pick a ready docs task and complete it.
```

## Connect chat or voice agents

A chat or voice agent can capture project-scoped tasks without access to a
repository checkout. Run `aven skill` to print Aven's reusable Markdown guidance
and include it in the integration's system prompt or tool instructions. Then let
the integration invoke the CLI with an explicit project and workspace.

A synchronized integration typically:

1. Runs `aven sync` to receive recent project and task data.
2. Converts the message or transcript into a title and useful description.
3. Creates the task with `aven add`, capturing the returned reference.
4. Runs `aven sync` again so the task reaches other devices.
5. Returns the reference to the person who requested the task.

For example, a private Telegram agent on a small home server can transcribe a
voice message, map the spoken project name to an Aven project, and pass the
result to a wrapper implementing that sequence. Use explicit allowlists for
workspaces and projects, and keep authentication tokens and other secrets out of
task content.

See [Sync across devices](/sync/) for server and client setup. Use `aven doctor`
to verify the integration's database, workspace, and project routing.

## Durable handoff

Descriptions hold the main task context: problem statement, scope, acceptance criteria, and links.

Notes hold durable handoff context: implementation decisions, blockers, partial progress, and review findings. Agent notes survive chat sessions, branch switches, worktrees, and machine restarts.

:::caution[Protect durable context]
Keep secrets out of titles, descriptions, labels, projects, notes, and logs.
:::

## Skill or prime?

Use the installed skill and priming for different levels of commitment:

| Setup | What the agent receives | Choose it when |
| --- | --- | --- |
| Installed skill | Reusable aven instructions loaded on demand | You want the agent to know how to find tasks, update status, and leave handoff context when the request calls for task management, without adding open-task context to every session. |
| `aven prime` | Aven instructions plus live active, ready, and blocked work for the current project | You want the agent to see current task context as part of session startup. |
| Both | On-demand aven knowledge everywhere, and live task context in selected sessions | Aven is part of your workflow, but only some projects need automatic task context. |

`aven prime` includes the same guidance as the installed skill. A prime hook is the best fit for projects where the agent should connect the current work to active, ready, or blocked tasks without being asked to run a command first. Installing the skill is the best fit for agent sessions where task-management requests should load aven guidance on demand, while live task context can be loaded only when useful.

A common setup is to install the skill globally, then add a project-local prime hook only in repositories tracked in aven. That keeps agents aware of aven everywhere and gives them open-task context where it is useful.

## What prime includes

`aven prime` is the agent bootstrap command. It prints the aven skill plus open work for the inferred project.

The open work is grouped by pickability:

| Group   | Meaning                                      |
| ------- | -------------------------------------------- |
| Active  | Work already in progress                     |
| Ready   | Open work with dependencies resolved         |
| Blocked | Open work waiting on unresolved dependencies |

This gives each agent session a current view of what is active, what can be picked up, and what is blocked.

A prime output includes the full skill first. The open-task part looks like this:

```txt
## Local Conventions

Project: aven
Open issue sample: 5
Use capitalized task titles.
Common statuses: active=2, inbox=3.
Common labels: keybindings=1, ux=1.

## Open Issues

Summary: total=5 active=2 ready=3 blocked=0
Top blockers: none.

### Active
AVN-RQ4N status=active title="Investigate task dependencies or epics"
AVN-F74G status=active title="Add full mouse support to the tui"

### Ready
AVN-Z55V status=inbox priority=high title="Add due dates or scheduling"
AVN-CDEQ status=inbox title="Documentation site"
AVN-12YM status=inbox labels=keybindings,ux title="Resolve Ctrl+P keybinding conflict"

### Blocked
(none)
```

## The aven skill

```sh
aven skill
```

`aven skill` emits the reusable agent-facing guidance for operating aven. It teaches agents how to use refs, inspect tasks, update status, create follow-up work, leave notes, handle long Markdown, and avoid unsafe task mutations.

`aven prime` includes this skill automatically, so humans usually do not need to run `aven skill` directly. The separate command is useful for debugging or custom agent integrations.

The source guidance lives in [`src/skill.md`](https://github.com/raine/aven/blob/main/src/skill.md).
