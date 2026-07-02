---
title: Agents
description: Make aven available inside AI coding agent sessions.
---

aven gives AI coding agents access to the same local task system you use in the TUI. The TUI is the human surface. The CLI is the agent surface.

For a human setting this up, the model is simple:

1. `aven skill` teaches an agent how to use aven.
2. `aven prime` combines that skill with live open-task context.
3. Your agent environment can run `aven prime` automatically when a session starts.

After that, you can ask an agent to work from aven tasks without explaining the task model or pasting command instructions into every prompt.

## Automatic priming

`aven prime` is the agent bootstrap command. It prints the aven skill plus open work for the inferred project.

The open work is grouped by pickability:

| Group | Meaning |
| --- | --- |
| Active | Work already in progress |
| Ready | Open work with dependencies resolved |
| Blocked | Open work waiting on unresolved dependencies |

This gives each agent session a current view of what is active, what can be picked up, and what is blocked.

A prime output includes the full skill first. The open-task part looks like this:

```txt
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

## Agent session setup

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

aven infers the active workspace and project from the current directory. Start an agent from a repository directory and `aven prime` loads the matching project context.

Use [Configuration](/configuration/) when directory names, workspace routes, or project path mappings need to be explicit. Use `aven doctor` from the same directory to inspect the active database, workspace, project, and routing decisions.

## The aven skill

```sh
aven skill
```

`aven skill` emits the reusable agent-facing guidance for operating aven. It teaches agents how to use refs, inspect tasks, update status, create follow-up work, leave notes, handle long Markdown, and avoid unsafe task mutations.

`aven prime` includes this skill automatically, so humans usually do not need to run `aven skill` directly. The separate command is useful for debugging or custom agent integrations.

The source guidance lives in [`src/skill.md`](https://github.com/raine/aven/blob/main/src/skill.md).

## Human workflow

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

## Durable handoff

Descriptions hold the main task context: problem statement, scope, acceptance criteria, and links.

Notes hold durable handoff context: implementation decisions, blockers, partial progress, and review findings. Agent notes survive chat sessions, branch switches, worktrees, and machine restarts.

Keep secrets out of titles, descriptions, labels, projects, notes, and logs.
