---
title: Recurring tasks
description: Set up work that comes back on a schedule and manage it over time.
---

Some work comes back: a daily journal, a Friday review, or a monthly invoice
check. A recurring task remembers the schedule and the details to reuse, while
Aven gives you one unfinished task for the relevant date instead of filling
your queue with copies.

Each dated task behaves like any other Aven task. You can edit it, add notes or
attachments, mark it done, or skip it. Changes to that task stay with that date.
Use **Edit future tasks** when later tasks should use different details.

## Create a recurring task

### In the TUI

Press `a` to open the task composer, then type a repeating expression in
**Schedule**:

- `daily`
- `every weekday at 09:00`
- `every Friday at 09:00`
- `fortnightly`
- `every 3 weeks, no due`
- `monthly, starting 2026-08-01`

Press `Enter` on **Schedule** to use the structured editor instead. Choose
**Repeating**, then set the repetition pattern, availability time, due setting,
and start date. TUI schedules use the device time zone.

### From the command line

Use `aven add` with `--repeat`:

```sh
aven add "Daily journal" \
  --repeat daily \
  --repeat-at 09:00 \
  --time-zone Europe/Stockholm

aven add "Review invoices" \
  --repeat monthly \
  --repeat-start-on 2026-01-31
```

Creation prints two references. The `RCR-` reference identifies the whole
recurring task. The ordinary task reference identifies the task for one
scheduled date.

## Choose a schedule

The TUI accepts friendly expressions. The CLI uses a fixed grammar so scripts
remain predictable.

| Schedule | TUI example | CLI value for `--repeat` |
| --- | --- | --- |
| Every day | `every day` | `daily` |
| Monday through Friday | `every weekday` | `weekdays` |
| Weekly | `every Friday` | `weekly on fri` |
| Every two weeks | `fortnightly` | `fortnightly` |
| Selected weekdays | `every Monday and Thursday` | `weekly on mon,thu` |
| Every N weeks | `every 3 weeks` | `every 3 weeks` |
| Selected days every N weeks | `every 3 weeks on Monday and Thursday` | `every 3 weeks on mon,thu` |
| Monthly | `every month` | `monthly` |

Weeks begin on Monday. `weekly` and `fortnightly` without an explicit weekday
use the start date's weekday.

Monthly schedules keep their original day number. When a month is shorter,
Aven uses its last day without changing the schedule. A task that starts on
January 31 returns on February 28 or 29, then March 31.

## Choose when tasks become available and due

The scheduled date controls which day a task belongs to. An optional
availability time controls when it enters normal task lists on that date.
Before then, you can find it under Upcoming.

Recurring tasks use one of two due settings:

- **Same day** makes the scheduled date the due date.
- **No due date** leaves each task without a deadline.

The CLI options are `--repeat-at`, `--repeat-due`, `--repeat-start-on`, and
`--time-zone`. See [`aven add`](/command-reference/#aven-add) for their exact
formats and defaults.

## Finish, skip, or miss a task

When you finish the current task, Aven creates the next one immediately and
defers it to its scheduled date. It stays out of your queue until it becomes
available, but you can see it early under Upcoming.

Canceling or choosing **Skip** means you are not doing this one. Aven records
the scheduled date as skipped, then continues with the next task. Use Skip for
"not this time." Use Stop when the work should never repeat again.

If a scheduled date passes without being finished or skipped, Aven records it
as missed and moves on. Missed work stays in the recurring task's history
instead of accumulating in your queue.

Aven does not need to remain open overnight. When you return after several
scheduled dates have passed, the next task-list or recurring-task view catches
up and shows the task for the relevant date.

The current task cannot be deleted, and a finished or skipped task cannot be
reopened later. In the TUI, use Undo immediately after an accidental change.
Otherwise, manage the recurring task with Skip, Pause, or Stop so its history
remains meaningful.

## Pause, resume, or stop

Open **Recurring Tasks** from the TUI sidebar or press `v u`. Select a recurring
task to inspect its schedule and use its actions.

| Action | What happens |
| --- | --- |
| **Pause** | Hides the current task and suppresses scheduled dates until you resume. |
| **Resume** | Continues the schedule without creating tasks for the paused period. |
| **Stop** | Ends the schedule permanently and leaves the current task available as the last one. |
| **Stop and skip current** | Ends the schedule and records the current task as skipped. |

If you resume while the preserved task's scheduled date is still relevant, it
returns. If that date has passed, Aven continues from the next applicable date.

The CLI offers the same controls:

```sh
aven recur pause RCR-7KP2
aven recur resume RCR-7KP2
aven recur stop RCR-7KP2
aven recur stop RCR-7KP2 --skip-current
```

## Change future tasks

**Edit future tasks** changes the reusable settings for tasks created later. It
does not rewrite the current task or history.

You can change the title, description, project, starting status, priority,
labels, availability time, and due setting. The repetition pattern, start date,
and time zone define the schedule and cannot be edited. Stop the recurring task
and create another one when those properties need to change.

From the CLI, use `aven recur edit`. A recurring-task reference or any linked
task reference identifies the recurring task.

## Review history

The history shows completed, skipped, and missed dates along with pause periods.
Open a recurring task in the TUI and choose **Show history**, or run:

```sh
aven recur history RCR-7KP2
```

Done lists and search results group past dated tasks by recurring task. Pass
`--expand-recurring` to `aven list` or `aven search` when you want every dated
task as a separate row.

## Sync across devices

Recurring tasks are safe to use on several synchronized devices. The schedule,
each dated task, and completed, skipped, or missed history sync with the rest of
your task data. When two devices create the task for the same scheduled date,
Aven recognizes it as the same task instead of keeping duplicates.

Completing the same dated task on two devices merges without requiring a
choice. If one device completes it while another skips it, Aven keeps the next
task moving forward and records a conflict so you can choose the correct
history. Conflicting Pause, Resume, or Stop changes also appear for review and
prevent additional tasks until resolved.

See [Resolve conflicts](/sync/#resolve-conflicts) for the review workflow.

## Command and shortcut reference

See [`aven recur`](/command-reference/#aven-recur) for every CLI command, option,
and output format. See [TUI](/tui/#recurring-tasks) for the complete keyboard
shortcut table.
