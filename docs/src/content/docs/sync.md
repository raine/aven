---
title: Sync across devices
description: Synchronize Aven data with end-to-end encryption, resolve conflicts, and diagnose sync state.
---

Sync keeps the same aven tasks available across laptops, agents, and other devices. Each client writes to its own local SQLite database first, so task capture and updates stay fast and offline-friendly.

Sync is end-to-end encrypted. Devices encrypt tasks, history, and images before
upload; the self-hosted server stores ciphertext and cannot read your data. One
device starts the sync from its database, and each other device joins with an
invitation from a device that already syncs.

A database that has not been set up or joined stays local. Everything except
sync keeps working.

In the TUI, the **Sync** dialog covers the same steps: open it with `:sync`,
`C s`, or a click on the sync indicator in the header. Press `S` to sync
immediately without opening it.

Use [Configuration](/configuration/) for `sync.*` and `daemon.*` settings. See
[Back up and restore](/backups/) when you need to preserve, move, or recover
local data.

## Start a server

Prepare server storage once, giving the URL devices will use to reach it:

```sh
aven server setup --data ~/.local/state/aven/sync-server.sqlite --url https://sync.example.com
```

The command prints a setup invitation. It expires after one hour; until a
device claims the server, running setup again replaces it. Anyone with the
invitation can claim the server, so hand it only to the device whose data
should start the sync.

Then serve the storage:

```sh
aven server --data ~/.local/state/aven/sync-server.sqlite --bind 127.0.0.1:3746
```

The server binds only loopback addresses and does not terminate TLS. Put a TLS
reverse proxy in front of it for other devices. The URL passed to `server setup`
must be an origin such as `https://sync.example.com`, or `http://` with a
loopback address; it has no path, query, or credentials.

Server storage used by the unencrypted sync of earlier releases is not
supported. `aven server` and `aven server setup` refuse storage that holds
change history; prepare a new path instead.

Run `aven server` under your operating system's service manager after
confirming that sync works.

## Set up sync from one device

On the device whose data should start the sync, paste the setup invitation:

```sh
aven sync setup
```

Setup previews the database and asks for confirmation. When standard input is
not a terminal, pipe the invitation and pass `--yes`. Every other device starts
from this data. Afterwards the database can no longer use backup restore or
import. Rerun the same command to resume an interrupted setup.

In the TUI, choose **Set up sync** in the Sync dialog and paste the invitation.
The invitation is never displayed. Before anything starts, the dialog shows the
server, the workspaces and tasks this database will publish, and any images
missing on this computer, which other devices see as unavailable. Setup keeps
running if you close the dialog, and the header shows it is syncing. If setup
stops, for example because the server is unreachable, the dialog offers
**Resume setup**, which continues the same setup instead of starting over. After
restarting the TUI, resuming asks for the same invitation again.

## Add a device

On a device that already syncs, create a device invitation and keep the command
running:

```sh
aven sync invite
```

The command prints an `aven://pair/v2/` invitation and, in an interactive
terminal, also shows it as a QR code. In the TUI, choose **Add device** in the
Sync dialog, or `:add-device`, to show the QR code; the TUI keeps waiting for the
device after you close the overlay. To paste the invitation on another
computer, press `c` in the overlay to copy it; the TUI never copies it
otherwise. Treat the copied invitation like a password and clear the clipboard
after pasting.

On the new device, use an empty database and paste the invitation:

```sh
aven sync join
```

In the TUI, choose **Join existing sync** in the Sync dialog, paste the
invitation, and confirm the server. A database that already has tasks or other
data cannot join, because existing local data cannot be merged with synced data
yet; the dialog explains this and changes nothing. While joining waits for the
inviting device and downloads tasks, the TUI pauses adding tasks, projects,
labels, and workspaces. Once tasks arrive they appear in the list while images
keep downloading. **Resume joining** continues an interrupted join without the
invitation. If the inviting device does not add this one in time, keep it waiting and
resume. A timeout does not show whether the invitation expired. If it expired,
create a new invitation on the same inviting device and continue with it:
**Use a new invitation** in the TUI, or `aven sync join --new-invitation` on the
command line. This device keeps its identity, and the earlier invitation is
kept, so if the other device already added this one with it, joining finishes
with that admission. If the database reaches its invitation limit, resuming
still finishes if the other device added it with any retained invitation. If
none was accepted, or local data was added while joining, it cannot finish
joining; keep it as it is and join from a new, empty database.

Anyone with the invitation can access all synced data and manage devices. It
expires after ten minutes. Sync on the inviting device pauses until the
invitation is used; an unused invitation stops pausing sync once it expires. If
keys may have been sent to a device that never joined, the next sync after
expiry rotates keys first. That device can still read anything it received.

## Manage devices

List the devices that take part in sync:

```sh
aven sync device list
aven sync device list --json
```

The list comes from the server's current membership. It shows each device's
`device_id`, which device is the current one, and whether key rotation is
still pending after a removal.

Remove another device with its `device_id`:

```sh
aven sync device remove DEVICE_ID
aven sync device remove DEVICE_ID --json
```

Removal stops the device from syncing and rotates the keys for future
changes. The result is `complete`, or `pending` while key rotation is
unfinished; rerunning the command, or syncing any remaining device, finishes
it. If the command is interrupted, rerun it with the same `device_id` to
resume. Removal does not erase anything on the removed device: it keeps the
tasks and images it already downloaded. A device cannot remove itself.

In the TUI, choose **Manage devices** in the Sync dialog. The list is checked
with the server when it opens and says when it was checked. Each device appears
by the shortest ID prefix that tells it apart, and the current one is marked
**This device**; select a device to see its full ID, and press `y` to copy it.
Press Enter on another device and confirm to remove it. The dialog reports
access removal and key rotation separately, and offers **Finish removal** while
rotation is unfinished. A server refusal means the removal could not be
confirmed, not that it happened.

## Sync a client

```sh
aven sync
aven sync --json
```

Each sync exchanges bounded rounds until tasks are up to date and image
transfers settle. Sync always uses the server chosen during setup or join.

Check local state without contacting the server:

```sh
aven sync status
aven sync status --json
```

The status report shows whether the database is set up, the server, whether
local changes wait to sync, and pending image uploads and downloads. JSON is a
versioned report intended for scripts and omits keys, invitations, and task
content.

### Image attachments during sync

Images sync after their tasks. If sync reports that images are still
transferring, run `aven sync` again or let the running daemon finish in the
background. An image added here that is missing from this computer blocks later
changes from uploading until it is available.

## Automate sync with the daemon

The daemon performs background sync for the configured local SQLite database.

```sh
aven daemon
```

Daemon sync requires `sync.enabled = true`. The wake address must be loopback.
Until the database is set up or joined, the daemon waits without contacting any
server.

The daemon wakes after successful local mutations when possible, syncs
periodically, continues promptly while work remains, and backs off after
failures. Local edits do not bypass a pending retry delay. You can run
`aven sync` to retry immediately.

Inspect service installation, configuration, executable consistency, runtime,
and log paths without changing the service:

```sh
aven daemon status
aven daemon status --json
```

On macOS, install it as a user LaunchAgent:

```sh
aven daemon install
aven daemon restart
aven daemon uninstall
```

Package scripts can refresh an installed LaunchAgent after replacing the binary:

```sh
aven daemon repair --if-installed --program /path/to/aven
```

The repair command succeeds without changes when the LaunchAgent is absent.

## Recover from device loss

A database that has set up or joined sync refuses `aven backup restore` and
`aven import`. This is intentional: do not replace data in a database that
takes part in sync.

If one device is lost or broken, no backup is needed. Run `aven sync invite` on
a device that still syncs, then run `aven sync join` on an empty database on
the replacement device. See [Add a device](#add-a-device) for the full steps.

If every syncing device is lost, restore the backup to a fresh database path:

```sh
aven --db /path/to/recovered.sqlite backup restore backup.aven-backup.tar.zst --yes
```

The restored data is available locally immediately. To sync it again, prepare
a new server data path and serve it:

```sh
aven server setup --data /path/to/new-sync-server.sqlite --url https://sync.example.com
aven server --data /path/to/new-sync-server.sqlite --bind 127.0.0.1:3746
```

Then use the setup invitation to start a new sync from the restored database:

```sh
aven --db /path/to/recovered.sqlite sync setup
```

Do not reuse the old sync's server storage. Join every other device to the new
sync from an empty database. Before discarding any old database you can still
access, check it for changes made after the backup that never synced; those
changes are not carried into the new sync.

See [Back up and restore](/backups/) for backup contents and restore behavior.

## Resolve conflicts

Conflicts happen when multiple clients edit the same task field between syncs. They are explicit and field based. Inspect conflicts before resolving them.

```sh
aven conflict list
aven conflict show APP-7KQ9 --field description
aven conflict diff APP-7KQ9 description
aven conflict export APP-7KQ9 description --dir conflicts
aven conflict resolve APP-7KQ9 description --use local
```

Use `--value`, `--value-file`, or `--value-stdin` when neither variant is the desired final value.

The TUI also has a conflicts view and conflict actions for human review.

## Diagnose sync state

```sh
aven doctor
aven doctor --integrity
aven doctor --json
```

For sync specifically, doctor reports whether the database is set up, the sync cursor, pending changes, conflicts, daemon wake validity, and integrity status when requested. A missing current recurring task is repairable by running `aven recur list`. Other recurring-task integrity failures require preserving the database and recovering from a known-good backup.

## Task source compatibility

Task origins are `cli`, `tui`, `api`, `ios`, `android`, and `unknown`. New iOS queue
captures use `ios`; generic API creation uses `api`. The `android` value is
reserved as a known client origin, not an indication that an Android app ships.
Existing sources stay unchanged because their original client cannot be reliably
inferred. Missing source values in older sync records and exports default to
`unknown`. Unrecognized values are rejected. Upgrade clients before opening an
upgraded database or importing an export with an unsupported source. Keep a
backup before upgrading if you need to return to an older release. Source is immutable, not an editable field or a
dedicated list filter. Metadata named `source` is independent of task origin.
