---
title: Changelog
description: Release notes for aven.
---

## v0.1.10 (2026-07-16)

- Set task availability to defer work until a date or time, and find deferred
  tasks in the Upcoming view.
- Add due dates independently of availability, with overdue filtering and
  deadline-aware queue ordering for visible, actionable tasks.
- The documentation includes a complete CLI command reference and a practical
  VPN sync setup guide.

## v0.1.9 (2026-07-15)

- The TUI automatically checks for new releases and offers guided installation,
  with tailored instructions for package-managed installations.
- `aven update` checks for and installs application updates. Task field changes
  use `aven edit <ref>`, a breaking command rename for scripts and agents.

## v0.1.8 (2026-07-15)

- [Task detail views](/tui/#select-and-copy-text) support dragging across
  rendered titles and descriptions and copying the selected text with `y`.
- [`Ctrl-Enter`](/tips/#use-ctrl-enter-in-alacritty-and-tmux) submits task
  composers and multiline editors, with `Ctrl-s` as a portable fallback.
- [Batch actions](/tui/#triage-and-edit-tasks) clearly indicate when they target
  marked tasks, and status changes use the status picker.
- [Multiline editor controls](/tui/#capture-tasks) remain visible as content
  grows, and note prompts appear only on empty drafts.

## v0.1.7 (2026-07-15)

- Move selected or marked tasks directly between
  [Columns lanes](/tui/#columns-view) with keyboard or mouse controls, with
  batch moves grouped into one undo step.
- Improved the default [Columns workflow](/tui/#columns-view) with
  lifecycle-based lanes, ordering tailored to each lane, and a toggle for the
  selected-task preview.
- Task previews render Markdown formatting and clearly indicate truncated text.
- Configured [database, workspace, and project paths](/configuration/) support
  `~` for the home directory.

## v0.1.6 (2026-07-14)

- Added a configurable [Columns view](/tui/#columns-view) for navigating tasks
  in named status lanes.
- Redesigned the [task composer](/tui/#capture-tasks) so project, status,
  priority, labels, title, and description stay visible and are accessible by
  keyboard or mouse.
- Added task copy shortcuts for refs, titles, descriptions, notes, and combined
  task text.
- Improved [task detail navigation](/tui/#open-detail) with a shortcut that
  jumps directly to notes.
- Added a guided
  [Taskwarrior migration workflow](/taskwarrior/#migrate-from-taskwarrior) to
  the documentation.
- Improved [daemon and server startup](/sync/) in Linux services, containers,
  and other environments without writable state directories.

## v0.1.5 (2026-07-12)

- Selecting an [epic](/concepts/#dependencies-and-epics) in the task list
  highlights its child tasks for easier scanning.

## v0.1.4 (2026-07-09)

- [Task composer](/tui/#capture-tasks) labels can be set while creating a task.
- TUI timestamps display in local time across task details, notes, recent
  actions, and database stats.
- [Database diagnostics](/sync/#diagnose-sync-state) show sync history size,
  synced and pending change counts, server sequence range, and payload bytes.
- [Task list](/tui/#screen-tour) selection stays visible when queue group
  headers remain pinned.
- Back navigation from an
  [epic child detail](/tui/#projects-labels-dependencies-and-epics) returns to
  the parent detail view and scroll position.

## v0.1.3 (2026-07-07)

- [Epic detail views](/tui/#projects-labels-dependencies-and-epics) list child
  tasks inline, with mouse hover and click targets for jumping to visible child
  tasks.
- The [queue](/concepts/#queue) keeps blocked tasks below actionable groups and
  gives more weight to tasks that unblock other work.
- The [TUI sidebar](/tui/#screen-tour) preserves selection more consistently
  when switching focus, toggling the sidebar, or applying task changes.
- Crashes are written to the aven log file with panic details for easier
  troubleshooting.

## v0.1.2 (2026-07-05)

- Added [`aven skill install`](/agents/#install-the-aven-skill) so Claude Code,
  OpenCode, and Codex can install the bundled task-management skill into their
  agent skill directories.
- Added a [Recent Actions view](/tui/#find-work) in the TUI for reviewing task,
  project, label, and dependency activity.
- Improved the [TUI queue](/concepts/#queue) with epic parent and child context,
  a `SOON` band, created-age metadata, tighter task and label column sizing, and
  consistent metadata spacing.
- Added [batch editing](/tui/#triage-and-edit-tasks) in the TUI: mark multiple
  tasks, then apply one status change or add and remove labels across the
  selected tasks in a single action.
- Improved [TUI navigation and controls](/tui/#keyboard-reference) with back
  navigation, footer status hotkeys, mouse-scrolled help overlays, and clearer
  terminal startup errors.
- Improved [task detail and preview panels](/tui/#open-detail) with epic
  metadata and parent markers.

## v0.1.1 (2026-07-03)

- Made natural task creation from the [full TUI](/tui/#capture-tasks) continue
  reliably after exiting the interface, while keeping created tasks undoable
  when the TUI remains open.
- Improved [daemon update handling](/sync/#automate-sync-with-the-daemon) so
  launchd services use a stable executable path, restart cleanly, and show
  richer status in `aven doctor`.

## v0.1.0 (2026-07-02)

- Initial release of `aven`, a local-first task manager. See
  [Getting started](/getting-started/) to install it and create your first task.
