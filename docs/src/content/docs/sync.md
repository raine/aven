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

## Add a device

On a device that already syncs, create a device invitation and keep the command
running:

```sh
aven sync invite
```

The command prints an `aven://pair/v2/` invitation and, in an interactive
terminal, also shows it as a QR code. In the TUI, open the command panel with
`:` and choose `:add-device` to show the QR code; the TUI keeps waiting for the
device after you close the overlay.

On the new device, use an empty database and paste the invitation:

```sh
aven sync join
```

Anyone with the invitation can access all synced data and manage devices. It
expires after ten minutes. Sync on the inviting device pauses until the
invitation is used; an unused invitation stops pausing sync once it expires. If
keys may have been sent to a device that never joined, the next sync after
expiry rotates keys first. That device can still read anything it received.

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

## Back up and move data

Backup, restore, export, and import guidance lives in
[Back up and restore](/backups/).

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
