# Changelog

## v0.1.5 (2026-07-12)

- Selecting an epic in the task list highlights its child tasks for easier
  scanning.

## v0.1.4 (2026-07-09)

- Task composer labels can be set while creating a task
- TUI timestamps display in local time across task details, notes, recent
  actions, and database stats.
- Database diagnostics show sync history size, synced and pending change counts,
  server sequence range, and payload bytes.
- Task list selection stays visible when queue group headers remain pinned.
- Back navigation from an epic child detail returns to the parent detail view
  and scroll position.

## v0.1.3 (2026-07-07)

- Epic detail views list child tasks inline, with mouse hover and click targets
  for jumping to visible child tasks.
- The queue keeps blocked tasks below actionable groups and gives more weight to
  tasks that unblock other work.
- The TUI preserves sidebar selection more consistently when switching focus,
  toggling the sidebar, or applying task changes.
- Crashes are written to the aven log file with panic details for easier
  troubleshooting.

## v0.1.2 (2026-07-05)

- Added `aven skill install` so Claude Code, OpenCode, and Codex can install the
  bundled task-management skill into their agent skill directories.
- Added a Recent Actions view in the TUI for reviewing task, project, label, and
  dependency activity.
- Improved the TUI queue with epic parent and child context, a `SOON` band,
  created-age metadata, tighter task and label column sizing, and consistent
  metadata spacing.
- Added batch editing in the TUI: mark multiple tasks, then apply one status
  change or add and remove labels across the selected tasks in a single action.
- Improved TUI navigation and controls with back navigation, footer status
  hotkeys, mouse-scrolled help overlays, and clearer terminal startup errors.
- Improved task detail and preview panels with epic metadata and parent markers.

## v0.1.1 (2026-07-03)

- Made natural task creation from the full TUI continue reliably after exiting
  the interface, while keeping created tasks undoable when the TUI remains open.
- Improved daemon update handling so launchd services use a stable executable
  path, restart cleanly, and show richer status in `aven doctor`.

## v0.1.0 (2026-07-02)

- Initial release of `aven`, a local-first task manager.
