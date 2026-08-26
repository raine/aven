---
title: Changelog
description: Release notes for aven.
---

## v0.1.34 (2026-08-26)

- Drag task cards between status lanes in the [Columns layout](/tui/#columns).

## v0.1.33 (2026-08-25)

- Fix: Sync keeps working when a recurring series projects an occurrence into a deleted project, and deleting a project stops its recurring series while keeping already projected tasks. ([#15](https://github.com/raine/aven/issues/15))
- Fix: Pressing `Enter` on a highlighted [sidebar](/tui/) row selects that view, project, or workspace again instead of opening the detail of the task selected in the list. ([#16](https://github.com/raine/aven/pull/16))
- Fix: [TUI sorting, filtering, and compatible view changes](/tui/#search-filter-and-order) keep the same task selected when it remains visible, with predictable nearby selection when it does not.

## v0.1.32 (2026-08-24)

- Running `aven` with no command opens the [TUI](/tui/) directly. Global options such as `--db` and `--workspace` still apply.
- Use [Columns](/tui/#columns) as a layout for the active task view. Press `v l` to switch between Columns and List without changing which tasks are shown.
- UX: The [TUI command palette](/tui/#discover-commands) prioritizes actions for the focused task, marked tasks, detail row, or sidebar item and keeps that target through pickers and confirmations.
- UX: [Task list](/tui/#find-work) selection uses a stable `›` cursor so the focused task remains clear across terminal themes, inactive panes, and monochrome output.

## v0.1.31 (2026-08-23)

- New: With [Related tasks](/organize-tasks/#connect-context-with-related-tasks), you can link tasks that share context without making one block the other or putting them in the same epic. Links appear on both tasks and can be managed from the CLI or TUI.
- UX: Workspace controls keep workspaces in a stable order, and clicking the header switches directly when only two workspaces are available.
- UX: Undoable TUI changes now offer `u undo` in toasts, while the command panel and shortcut help describe the operation and affected task count before it is undone.

## v0.1.30 (2026-08-19)

- [Recurring tasks](/recurring-tasks/) support every N days, weeks, months, or years, including anchored month-end and leap-day behavior without schedule drift
- The [TUI task composer](/tui/#capture-tasks) automatically sets prioritized and repeating tasks to todo. Choose a status manually to override it, or pick `Auto` to restore the automatic choice.
- [Search](/tui/#search-filter-and-order) finds a recurring series by the `RCR-` ref shown on its task rows. ([#14](https://github.com/raine/aven/pull/14))
- Fix: Searching a task ref whose project prefix contains digits, such as `0M-XYMT`, finds the task, and accepting a search result in the TUI keeps showing it. ([#13](https://github.com/raine/aven/pull/13))
- Fix: A [custom TUI command](/custom-commands/) that exits early reports its own exit status instead of an input delivery error, and timed-out commands clean up leftover child processes.

## v0.1.29 (2026-08-12)

- Configure experimental [custom TUI commands](/custom-commands/) to run local programs from the command palette or keybindings and pass selected or marked task context.
- Fix: CJK, emoji, and other wide-character text fits correctly in TUI fields and layouts, and input method composition appears at the active caret. ([#10](https://github.com/raine/aven/pull/10))

## v0.1.28 (2026-08-10)

- Sync is more reliable on slow or unstable connections, coordinates daemon and manual operations, and downloads only attachments referenced by live tasks.
- Fix: The changelog reader (`:changelog`) fetches the latest release notes independently of cached update checks and wraps entries cleanly to the dialog width.

## v0.1.27 (2026-08-09)

- Task details show the project key and color beside the task reference
- Parent epic rows in child task details include the epic star marker for easier recognition at a glance.
- The bundled [Aven agent skill](https://github.com/raine/aven/blob/main/src/skill.md) heavily trimmed down from cruft

## v0.1.26 (2026-08-09)

- Add [task metadata](/task-metadata/) with `--metadata KEY=VALUE`, exact and presence filters, bulk updates, field listing and renaming, and full text, JSON, and Markdown detail output. Metadata values are opaque strings, and renaming a field preserves its stable identity without rewriting every task.
- The [Epics view](/organize-tasks/#group-an-outcome-with-an-epic) summarizes child completion, ready, blocked, overdue, and recent activity directly in parent rows, with responsive progress such as `1/5 done` and distinct canceled counts.
- Epic detail views show [unresolved blockers](/organize-tasks/#put-work-in-order-with-dependencies) beneath affected child tasks.
- Open the Recurring Tasks view directly with `aven tui --view recurring`.
- Read and update supported non-secret settings with `aven config get` and `aven config set`, including sync, update-check, database-path, and image optimization settings. Updates preserve comments and unrelated settings.

## v0.1.25 (2026-08-07)

- Fix: [Sorting](/tui/#search-filter-and-order) by updated time shows the most recently updated tasks first by default.
- Fix: [Task details](/tui/#task-detail) stay in sync after changes, including status and notes, while preserving the scroll position.

## v0.1.24 (2026-08-05)

- New: Press `Ctrl-g` in the [task composer](/tui/#capture-tasks) to create a task and immediately start another with the same project, status, priority, and labels.
- [Marked tasks](/tui/#mark-and-change-several-tasks) open a dedicated bulk-action mode that highlights compatible commands, explains single-task restrictions, and clears all marks with `Esc`.
- [Copy references, durable IDs, or titles](/tui/#copy-task-information) for marked tasks as one value per line in their visible list order.
- Browse [command palette](/tui/#discover-commands) results with the arrow keys and `Enter`, cycle the full catalog with `Tab` or `Shift-Tab`, and track your position with a scrollbar.
- Empty task views explain better why no tasks are shown and suggest relevant actions.

## v0.1.23 (2026-08-04)

- New: [Copy a complete task report as Markdown](/tui/#copy-task-information) with `y m`, or [publish it as a secret GitHub gist](/tui/#publish-a-task-report-as-a-github-gist) with `t g` and copy the resulting link.
- New: Click web links in task descriptions and notes to open them in the default browser, while link labels remain compact and readable.
- New: Navigate forward after going back with `g ]`, with list selection, scroll position, and task detail state preserved.
- New: [Disable automatic release checks](/configuration/#automatic-update-checks) and update badges with `update.automatic_checks: false` or `AVEN_NO_UPDATE_CHECK=1`; manual update commands remain available.
- New: Type immediately in [project and workspace pickers](/tui/#scope) to filter options, then use the arrow keys and `Enter` to select one.

## v0.1.22 (2026-08-03)

- Blocked tasks are dimmed in lists and Columns, help headings have clearer visual hierarchy, and nested dialogs render with faster, more consistent background dimming.
- Routine view, scope, filter, ordering, and workspace changes avoid redundant success messages when the updated state is already visible.

## v0.1.21 (2026-08-01)

- New: Create and edit epics in the TUI, or promote an ordinary task when adding its first child.
- New: Click any editable metadata value in task details to change the project, status, priority, labels, availability, or due date.
- New: Multiline editors confirm before discarding changed content, and individual notes can be edited, deleted, and restored with undo.
- New: Create, browse, rename, and delete labels, create and rename workspaces, manage project paths, and create projects from task workflows in the TUI.
- New: Save attachments from task detail with explicit handling for missing data, invalid destinations, and existing files.
- New: Open Upcoming from the shared command catalog, open tasks from Recent Actions, and receive series-specific guidance for task commands in the Recurring view.
- New: TUI mutations wake the sync daemon, `:sync` starts a manual sync, and sync-disabled environments preserve local-only operation with clear errors.
- Fix: Task detail commands consistently target the displayed or focused related task, with direct actions for linked tasks and reliable back and sibling navigation.
- Fix: TUI task and label workflows apply their changes atomically and restore labels created by the same operation during undo.
- Fix: Trackpad scrolling stays responsive at list and help boundaries, while modal dialogs and help overlays present clearer visual structure.
- Fix: Consecutively deleted tasks remain visible in the TUI until an explicit refresh.

## v0.1.20 (2026-07-31)

- [Create and manage recurring tasks](/recurring-tasks/) from the CLI or TUI with flexible schedules, history and lifecycle controls, cross-device sync, and portable backup and export workflows.
- Open release notes with `:changelog`, by clicking the version in the header, or inside the update dialog.
- [Search within one project](/command-reference/#aven-search) with `aven search --project`. JSON results include the same task fields as list output.
- [Bulk updates](/command-reference/#aven-bulk-update) include matching tasks scheduled for a future date. If any update fails, no selected tasks are changed.
- Set task status during creation and list open work available now with [`aven add --status`](/command-reference/#aven-add) and [`aven list --open`](/command-reference/#aven-list).
- Perf: Large task lists, recurring-task refreshes, live searches, and TUI rendering stay more responsive.
- [Epics](/concepts/#dependencies-and-epics) start collapsed, selected-task previews show due dates, and active filters remain visible in constrained TUI headers.
- Nested dialogs have clearer visual depth, and task composers stay compact while adapting to longer descriptions and schedule editors.
- [Detached natural-language task creation](/tui/#create-with-ai) sends a system notification when it finishes after the originating TUI exits.
- Fix: [Attachment sync](/sync/#image-attachments-during-sync) accepts UTC timestamps with fractional seconds.
- Out-of-range `--limit` values return clear errors for list, search, and [recurrence history](/command-reference/#recur-history).

## v0.1.19 (2026-07-28)

- Long task titles wrap in the detail pane and remain fully selectable and copyable.
- Natural-language task intake respects an explicitly selected project, keeps directory-based project inference available, and closes the composer after creating a task with an image.
- Press `g .` to return to a task after moving it out of the active project.
- Epics show accurate project-scoped counts, support viewing closed items with `f d`, and expand or collapse with `l` or Right.
- Selecting an epic child highlights its visible parent for clearer context.
- Exact command palette matches are selected immediately, and repeated `Tab` presses cycle through the other matches.
- Deleted tasks remain visible during background refreshes so they can be reviewed or restored until the view is explicitly refreshed.
- Note discard confirmations render cleanly without overlapping the editor.

## v0.1.18 (2026-07-27)

- Copy task text from the Linux TUI on Wayland or X11 with `wl-copy` or `xclip`.
- Sync failures include useful server details and specific guidance for authentication, configuration, rate limits, and server errors. Protocol mismatches identify whether the client or server needs an upgrade.
- Task changes and their undo history are saved together, preventing partial updates when a write fails.
- The TUI keeps task lists, detail views, selections, and edit targets consistent when an update fails or needs to be retried.

## v0.1.17 (2026-07-25)

- Manage epic children from the TUI by adding existing tasks, creating new tasks, removing links, and undoing relationship changes.
- Open linked tasks directly from detail views, including tasks outside the current filter, and return through your detail history with `Esc`.
- Edit the project, priority, availability, due date, or labels for marked tasks as a batch while keeping your marks available for follow-up changes.
- Note drafts with meaningful text ask for confirmation before `Esc` discards them.

## v0.1.16 (2026-07-23)

- Task detail help includes navigation shortcuts for moving through tasks, returning to previous views, and revisiting the most recently changed task.

## v0.1.15 (2026-07-23)

- Status changes keep your place in task lists, Columns, and detail views. If a changed task leaves the current view, press `g .` to return to it and then go back.

## v0.1.14 (2026-07-23)

- Explore Aven with `aven demo`, which opens a fresh sample workspace and discards all changes when you exit.
- Task details remain responsive while scrolling through inline image previews, which reappear as soon as scrolling settles.

## v0.1.13 (2026-07-21)

- [Attach images to tasks](/concepts/#image-attachments) from the TUI or CLI, sync them between devices, preview and switch between them inline, inspect their metadata, and open them in the system viewer.
- Click task references and timestamps in task details to copy their displayed values.

## v0.1.12 (2026-07-17)

- Search results and task details mark epics consistently, and child task details show their parent epic for clearer context.
- Navigate epic children from task details with `Tab`, `j`/`k` or the arrow keys, and `Enter`.

## v0.1.11 (2026-07-16)

- Due dates use compact labels in the task list so deadlines fit cleanly in the time column.

## v0.1.10 (2026-07-16)

- Set task availability to defer work until a date or time, and find deferred tasks in the Upcoming view.
- Add due dates independently of availability, with overdue filtering and deadline-aware queue ordering for visible, actionable tasks.
- The documentation includes a complete CLI command reference and a practical VPN sync setup guide.

## v0.1.9 (2026-07-15)

- The TUI automatically checks for new releases and offers guided installation, with tailored instructions for package-managed installations.
- `aven update` checks for and installs application updates. Task field changes use `aven edit <ref>`, a breaking command rename for scripts and agents.

## v0.1.8 (2026-07-15)

- [Task detail views](/tui/#copy-task-information) support dragging across rendered titles and descriptions and copying the selected text with `y`.
- [`Ctrl-Enter`](/tips/#use-ctrl-enter-in-alacritty-and-tmux) submits task composers and multiline editors, with `Ctrl-s` as a portable fallback.
- [Batch actions](/tui/#triage-and-edit-tasks) clearly indicate when they target marked tasks, and status changes use the status picker.
- [Multiline editor controls](/tui/#capture-tasks) remain visible as content grows, and note prompts appear only on empty drafts.

## v0.1.7 (2026-07-15)

- Move selected or marked tasks directly between [Columns lanes](/tui/#columns) with keyboard or mouse controls, with batch moves grouped into one undo step.
- Improved the default [Columns workflow](/tui/#columns) with lifecycle-based lanes, ordering tailored to each lane, and a toggle for the selected-task preview.
- Task previews render Markdown formatting and clearly indicate truncated text.
- Configured [database, workspace, and project paths](/configuration/) support `~` for the home directory.

## v0.1.6 (2026-07-14)

- Added a configurable [Columns view](/tui/#columns) for navigating tasks in named status lanes.
- Redesigned the [task composer](/tui/#capture-tasks) so project, status, priority, labels, title, and description stay visible and are accessible by keyboard or mouse.
- Added task copy shortcuts for refs, titles, descriptions, notes, and combined task text.
- Improved [task detail navigation](/tui/#task-detail) with a shortcut that jumps directly to notes.
- Added a guided [Taskwarrior migration workflow](/taskwarrior/#migrate-from-taskwarrior) to the documentation.
- Improved [daemon and server startup](/sync/) in Linux services, containers, and other environments without writable state directories.

## v0.1.5 (2026-07-12)

- Selecting an [epic](/concepts/#dependencies-and-epics) in the task list highlights its child tasks for easier scanning.

## v0.1.4 (2026-07-09)

- [Task composer](/tui/#capture-tasks) labels can be set while creating a task.
- TUI timestamps display in local time across task details, notes, recent actions, and database stats.
- [Database diagnostics](/sync/#diagnose-sync-state) show sync history size, synced and pending change counts, server sequence range, and payload bytes.
- [Task list](/tui/#find-work) selection stays visible when queue group headers remain pinned.
- Back navigation from an [epic child detail](/tui/#projects-and-relationships) returns to the parent detail view and scroll position.

## v0.1.3 (2026-07-07)

- [Epic detail views](/tui/#projects-and-relationships) list child tasks inline, with mouse hover and click targets for jumping to visible child tasks.
- The [queue](/concepts/#queue) keeps blocked tasks below actionable groups and gives more weight to tasks that unblock other work.
- The [TUI sidebar](/tui/#find-work) preserves selection more consistently when switching focus, toggling the sidebar, or applying task changes.
- Crashes are written to the aven log file with panic details for easier troubleshooting.

## v0.1.2 (2026-07-05)

- Added [`aven skill install`](/agents/#install-the-aven-skill) so Claude Code, OpenCode, and Codex can install the bundled task-management skill into their agent skill directories.
- Added a [Recent Actions view](/tui/#find-work) in the TUI for reviewing task, project, label, and dependency activity.
- Improved the [TUI queue](/concepts/#queue) with epic parent and child context, a `SOON` band, created-age metadata, tighter task and label column sizing, and consistent metadata spacing.
- Added [batch editing](/tui/#triage-and-edit-tasks) in the TUI: mark multiple tasks, then apply one status change or add and remove labels across the selected tasks in a single action.
- Improved [TUI navigation and controls](/tui/#discover-commands) with back navigation, footer status hotkeys, mouse-scrolled help overlays, and clearer terminal startup errors.
- Improved [task detail and preview panels](/tui/#task-detail) with epic metadata and parent markers.

## v0.1.1 (2026-07-03)

- Made natural task creation from the [full TUI](/tui/#capture-tasks) continue reliably after exiting the interface, while keeping created tasks undoable when the TUI remains open.
- Improved [daemon update handling](/sync/#automate-sync-with-the-daemon) so launchd services use a stable executable path, restart cleanly, and show richer status in `aven doctor`.

## v0.1.0 (2026-07-02)

- Initial release of `aven`, a local-first task manager. See [Getting started](/getting-started/) to install it and create your first task.
