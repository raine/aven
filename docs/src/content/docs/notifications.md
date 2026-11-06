---
title: Notifications
description: Get reminders for due and available tasks, and allow notifications during Focus.
---

Aven sends desktop notifications for:

- [reminders](#reminders) when tasks are due or become available, if you turn them on
- background AI task creation that finishes after you close the TUI

## Reminders

<div class="notification-example-light">
  <img src="/macos-reminder-digest-light.webp" alt="Illustration of an Aven daily digest notification titled 2 due today, 1 overdue, listing three tasks by ref and title" width="440" height="184" />
</div>
<div class="notification-example-dark">
  <img src="/macos-reminder-digest-dark.webp" alt="Illustration of an Aven daily digest notification titled 2 due today, 1 overdue, listing three tasks by ref and title" width="440" height="184" />
</div>

In Aven Desktop, turn on **Settings › Reminders › Reminders on this Mac**. Aven asks macOS to allow notifications, and a background agent keeps sending reminders after you quit the app. The Reminders page shows whether that agent is running and whether notifications are allowed. See [Aven Desktop in the background](#aven-desktop-in-the-background).

From the terminal, reminders come from the background daemon, so they arrive while the TUI and CLI are closed. Turn them on, and run `aven daemon install` if the daemon isn't installed yet. A running daemon picks up the change by itself:

```sh
aven config set reminders.enabled true
aven daemon install
```

On macOS, if Aven hasn't asked to send notifications yet, these commands ask and wait for your answer: choose **Allow** in the notification that appears. Turning on automatic sync in the TUI asks too when reminders are on. `aven daemon status` and `aven doctor` say whether notifications are allowed, denied, or not asked yet, and what to do.

Aven sends two kinds of reminder:

- **Daily digest:** once a day at `digest_time`, a summary titled with the counts, such as “2 due today, 1 overdue”, that lists the first three tasks by ref and title. Blocked and deferred tasks count too. Overdue tasks appear every day until you complete, cancel, or re-date them. If your Mac is asleep or off at `digest_time`, the digest arrives when it wakes later that day. A missed day's digest is never sent. If nothing is due, no digest is sent that day.
- **Available again:** when a deferred task reaches its available time, one notification for that task. A blocked task waits and notifies once its blockers are done. Deferring the task again to a new time notifies again at that time. When more than three tasks become available at once, you get a single summary listing the first three.

A task that became available more than 24 hours before the daemon noticed, for example after a long time offline, does not notify. With Aven Desktop installed, clicking a reminder about one task opens that task, and clicking the digest opens the Overdue view.

A reminder that can't be shown within a day is dropped instead of arriving late, and an undelivered digest is dropped at the end of its day. While notifications are off for Aven, reminders aren't shown; **Settings › Reminders** in Aven Desktop says how many weren't shown in the last week.

Each device keeps its own record of sent reminders. If several devices run the daemon, each one notifies.

Configure reminders in `config.yaml`. A running daemon or app picks up changes to the file:

```yaml
reminders:
  enabled: true
  digest_time: "09:00"  # local time, HH:MM
  due_digest: true      # daily digest of due and overdue tasks
  available: true       # deferred tasks becoming available
```

## Aven Desktop in the background

Aven Desktop sends reminders and syncs while it is open. To keep doing so after you quit it, it runs a background agent, listed as **Aven** under **System Settings → General → Login Items & Extensions**. The agent runs while either switch is on:

- **Settings › Reminders › Reminders on this Mac**
- **Settings › Sync and devices › Sync when Aven is quit**, offered on when you set up or join sync

Turn both off and Aven removes the agent. The first time you quit with one on, Aven says that they continue in the background.

If you turn Aven off in Login Items, Aven leaves the agent off and Settings says it was turned off there; turn it back on in Login Items to resume. `aven daemon uninstall` also turns the agent off for the app, and `aven daemon install` turns it on again. These choices are kept as `background: on` or `background: off` in `config.yaml`.

Aven Desktop requires macOS 13 or later.

## macOS

On macOS, Homebrew and the install script include an `Aven.app` bundle so notifications appear as **Aven**, with its own icon and notification settings.

<div class="notification-example-light">
  <img src="/macos-notification-light.webp" alt="Illustration of an Aven notification confirming the task Review the homepage copy was added" width="440" height="169" />
</div>
<div class="notification-example-dark">
  <img src="/macos-notification-dark.webp" alt="Illustration of an Aven notification confirming the task Review the homepage copy was added" width="440" height="169" />
</div>

Aven Desktop asks macOS to allow notifications when you turn on reminders, and the terminal commands above ask when you run them in a terminal. Choose **Allow**. To change this later, open **System Settings → Notifications → Aven**.

### Allow notifications during Focus

Open **System Settings → Focus**, select your Focus, and add **Aven** to its allowed apps. Repeat for each Focus where you want notifications.

### Notifications appear as Terminal

Nix and bare-binary installs send notifications as **Terminal**. Use Terminal’s notification and Focus settings instead.
