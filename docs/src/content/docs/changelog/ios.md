---
title: iPhone changelog
description: Release notes for Aven for iPhone.
---

## Unreleased

## 1.0.4 — in review

- **Tell Aven** turns a typed or spoken request into task drafts you review before anything changes. It can search and answer questions about your tasks, take a photo or screenshot as context, and open from task detail with that task attached. Chats stay on your iPhone.
- Voice input in Tell Aven offers on-device and alternative transcription providers, with a choice in Settings.
- Add notes to a task from task detail.
- Create new labels from the Add Task label picker and while editing a task.
- Task priority is drawn as signal bars.
- Aven runs on iOS 18 again.
- Sync retries brief failures before showing them.
- Fix: Switching tabs no longer flashes the screen.
- Fix: The add image tile lays out correctly in the task form.

## 1.0.3 (2026-10-28)

- Fix: HTTP sync connections to self-hosted servers over a trusted VPN, including Netbird, are no longer blocked for VPN addresses such as `100.100.20.30`.

## 1.0.2 (2026-10-17)

- Capture suggests labels as one-tap chips beside the Capture button. Labels matching words in the title come first, a chip reapplies the labels of the project's latest labelled task, and **…** opens the full label picker.
- The capture sheet has more room while typing: the title field replaces the "New task" heading, and the note under the Capture button is gone. Next to the label chips, the Capture button is a larger capsule that is easier to spot and tap.
- The label picker keeps selected labels visible as removable chips below the search field, and a **Frequent** section offers the project's most used labels. The edit form shows a task's labels as chips instead of a count.
- A **Feedback** section in Settings links to Aven's GitHub issues for suggestions and problem reports.
- **All tasks** shows open tasks by default. The status filter adds **Open tasks** and **All statuses** shortcuts, and priority and label filters move into submenus.
- Fix: Closing a capture you haven't changed no longer asks to discard it.
- Fix: The **All tasks** search field stays in place and visible while the keyboard is open, and scrolling back to the top shows search and filters again.

## 1.0.1 (2026-10-15)

- Sort, filter and search **All tasks** in a project: sort by date added, last updated, priority, due date or title; filter by status, priority and label; and pull down to search. The Tasks tab gets the same **All tasks** view for the whole workspace, next to Queue and Epics.
- Swipe tasks in **All tasks** like in the Queue: right to mark done, left to snooze until tomorrow, with Undo.
- Fix: Done and canceled tasks are no longer shown as overdue. With the due date sort, they appear under **Earlier** with their due date.
- Perf: **All tasks** opens smoothly with around a thousand tasks, and returning from a task no longer reloads the list.
- With an empty query, the Search tab shows the five most recently added tasks, with **Show all** to see the rest.
- Long-press a task to set its due date or snooze it with quick picks such as Tomorrow, Next week or Weekend, with Undo. The editor's Availability field is now called **Available from**.
- Create projects on iPhone: type a new name in the capture project picker or the Projects tab filter and choose **Create "…"**.
- Capturing a task in a workspace with no projects creates an **Inbox** project when you save, instead of asking you to sync a project first.
- Choose the default project for new tasks in Settings: the last one used (the default), none, or a specific project. Each device remembers it per workspace.
- Project colors match the terminal app, using 14 colors tuned for light and dark mode. The Projects list shows filled folder icons in each project's color.
- Join sync hosted by Aven Cloud or your own Aven server. Pairing and Settings show which one the phone uses.
- When sync has been deleted on the server, the app says so, stops syncing automatically, and shows how to disconnect and pair again. **Sync now** still checks.
- Joining with an invitation for a deleted sync explains that sync must be set up again on the computer, instead of reporting an expired invitation.
- Leaving a pairing that can still be retried asks before discarding it.
- Clearer sync errors: connection problems give advice that fits Aven Cloud or a self-hosted server, an app too old for its server says to update, hosting refusals (paused, storage limit, unavailable) explain what to do, and a reverse proxy that rejects large requests suggests raising its body limit to at least 8 MB.
- Fix: An invitation with an unusable server address asks for a new invitation instead of suggesting a network check.
- Fix: Swiping right on a task in a project marks it done instead of going back.

## 1.0 (2026-10-14)

- Initial release of Aven for iPhone, a companion to the [aven](https://aventasks.dev/) task manager. Capture tasks, work through the queue and epics, browse projects, search, and edit tasks with Markdown descriptions, notes, relationships and attachments.
- End-to-end encrypted sync with Aven on your computer: pair by scanning a QR code or pasting an invitation. Tasks stay available offline.
- Delete and restore tasks, with Undo.
- Open-source license notices are available offline, and Contact support opens Aven's support page.
- The app collects no data. Crash reports are only sent if you choose to email them.
