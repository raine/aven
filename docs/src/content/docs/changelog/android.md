---
title: Android changelog
description: Release notes for Aven for Android.
---

## Unreleased

## 0.2.0 — in review

- Capture suggests one-tap label chips while typing, prioritizing labels named in the title and frequently used in the project. Reuse the labels from the project's latest labelled task, or open the full picker with **…**.
- The label picker keeps selected labels visible as removable chips and offers frequently used labels. Capture and edit show selected labels as chips instead of a count.
- The capture title stays beside Close while the details scroll, replacing the separate **New task** heading.
- **All tasks** shows open tasks by default. **Open tasks** and **All statuses** shortcuts preserve other filters; priority and label choices have their own menu pages, and the filter button stays in place while selecting.
- Search focuses automatically the first time its tab opens, without reopening the keyboard when returning from a task.
- Settings has a **Suggest an improvement** link for bugs, ideas, and clunky workflows.
- Fix: Capture ignores whitespace-only changes when asking to discard a draft, and changing only its project now asks for confirmation.
- Sort, filter and search **All tasks** in a project: sort by date added, last updated, priority, due date or title; filter by status, priority and label; and pull down to search. The Tasks tab gets the same **All tasks** view for the whole workspace, next to Queue and Epics.
- Swipe tasks in **All tasks** like in the Queue: right to mark done, left to snooze until tomorrow, with Undo.
- Fix: Done and canceled tasks are no longer shown as overdue. With the due date sort, they appear under **Earlier** with their due date.
- Perf: **All tasks** opens smoothly with around a thousand tasks, and returning from a task no longer reloads the list.
- With an empty query, the Search tab shows the five most recently added tasks, with **Show all** to see the rest.
- Long-press a task to set its due date or snooze it with quick picks such as Tomorrow, Next week or Weekend, with Undo. The editor's Availability field is now called **Available from**.
- Create projects on Android: type a new name in the capture project picker or the Projects tab filter and choose **Create "…"**.
- Capturing a task in a workspace with no projects creates an **Inbox** project when you save, instead of asking you to sync a project first.
- Choose the default project for new tasks in Settings: the last one used (the default), none, or a specific project. Each device remembers it per workspace.
- Project colors match the terminal app, using 14 colors tuned for light and dark mode.
- Join sync hosted by Aven Cloud or your own Aven server. Pairing and Settings show which one the phone uses.
- When sync has been deleted on the server, the app says so, stops syncing automatically, and offers **Disconnect to pair again**. **Sync now** still checks.
- Joining with an invitation for a deleted sync explains that sync must be set up again on the computer, instead of reporting an expired invitation.
- Leaving a pairing that can still be retried asks before discarding it.
- Clearer sync errors: connection problems give advice that fits Aven Cloud or a self-hosted server, an app too old for its server says to update, hosting refusals (paused, storage limit, unavailable) explain what to do, and a reverse proxy that rejects large requests suggests raising its body limit to at least 8 MB.
- Fix: An invitation with an unusable server address asks for a new invitation instead of suggesting a network check.
- The capture dialog has more room while typing: the note under the Capture button is gone.

## 0.1.0 (2026-10-08)

- Initial release of Aven for Android, a companion to the [aven](https://aventasks.dev/) task manager. Capture tasks, work through the queue and epics, browse projects, search, and edit tasks with Markdown descriptions, notes, relationships and attachments.
- End-to-end encrypted sync with Aven on your computer: pair by scanning a QR code or pasting an invitation. Tasks stay available offline.
- Delete and restore tasks, with Undo.
- Open-source license notices are available offline, and Contact support opens Aven's support page.
- The app collects no data.
