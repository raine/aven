---
title: Back up and restore
description: Preserve, restore, or move Aven data safely.
---

Choose the command that matches what you need to preserve:

| Goal | Commands | Image files included? |
| --- | --- | --- |
| Preserve or move all local Aven data | `aven backup` and `aven backup restore` | Yes, for images available on this device when the backup is created |
| Move task data through JSON | `aven export` and `aven import` | No |

## Create and restore a complete backup

```sh
aven backup
aven backup --output backup.aven-backup.tar.zst
aven backup restore backup.aven-backup.tar.zst --yes
```

Without `--output`, Aven writes a timestamped backup beside the active database.
Use `--output` when you need a specific destination.

A backup archive contains the local data from a consistent SQLite snapshot and
every attachment image available on the device. Recurring schedules,
future-task settings, completed, skipped, and missed history, pauses, conflicts,
task fields, notes, and attachments remain intact. Sync bindings, enrollment
state, and encryption keys are excluded, so a restored copy is local only.

Images that have not downloaded cannot be included. Run `aven sync` first when
the sync server may have files this device lacks. Restore checks the archive and
images before replacing local data.

:::caution[Local data replacement]
Restore replaces local data and requires confirmation with `--yes`. Aven creates
a safety backup first and preserves the previous attachment directory.
:::

## Recover data used with sync

A database that has set up or joined end-to-end encrypted sync can create a
backup, including while setup or joining is incomplete. It still refuses
restore and import because replacing data in a database that takes part in sync
is unsafe. Restore the backup to a fresh database path instead.

If one device is lost or broken, use a surviving device instead of a backup:
run `aven sync invite` there, then run `aven sync join` on an empty database on
the replacement device.

If every syncing device is lost, restore the backup to a fresh database path:

```sh
aven --db /path/to/recovered.sqlite backup restore backup.aven-backup.tar.zst --yes
```

The restored data works locally immediately. To sync again, prepare a new
server data path with `aven server setup`, serve that storage, and run
`aven sync setup` against the restored database. Do not reuse the previous
sync's server storage. Other devices must join the new sync from empty
databases.

Changes made after the backup and never synced are not carried into the new
sync. Check any old database you can still access before discarding it. See
[Recover from device loss](/sync/#recover-from-device-loss) for the complete
command sequence.

## Export and import portable JSON

```sh
aven export --output tasks.json
aven import tasks.json --yes
```

A JSON export includes tasks, recurring schedules, future-task settings,
history, pauses, conflicts, and attachment information. It sets
`blobs_included: false` and leaves out image files.

Import validates recurring schedules, generated tasks, history, and recurring
state before replacing local data. Older task-only exports import every task as
nonrecurring data. Import keeps this installation's client identity and resets
the sync cursor and established compatibility behavior. When the export contains
history already accepted by a sync server, import retains that server binding so
Aven cannot silently treat the history as belonging to a different server.
Imported history must fit the maintained baseline; incompatible records are
rejected before local data is replaced. A SQLite backup preserves the database
relationship and its established behavior together.

:::caution[Local data replacement]
Import replaces local data and requires confirmation with `--yes`. Aven creates
a safety backup first.
:::

## Restore attachment images

After importing JSON, attachment labels remain visible while the TUI shows
unavailable-image placeholders. Sync can restore an image only if its attachment
was synchronized to the same server before the export. Until the bytes return,
[`attachment get --output`](/command-reference/#aven-attachment) cannot save the
image.

An attachment that was still local-only when exported remains local unavailable
metadata after import. Aven does not publish it because the JSON lacks the image
bytes required by the sync server. Use a backup archive instead when moving
unsynchronized images. Restoring that archive replaces the database and restores
the included image files together.

A complete backup includes only images available locally when it is created. If
you synchronize attachments, confirm that sync is complete before creating the
archive. See [Image attachments during sync](/sync/#image-attachments-during-sync).

See [Data safety commands](/command-reference/#data-safety-commands) for every
backup, restore, export, and import option.
