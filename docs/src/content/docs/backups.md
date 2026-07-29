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

A backup archive contains the complete SQLite database and every attachment
image available on the device. Recurring schedules, future-task settings,
completed, skipped, and missed history, pauses, conflicts, task fields, notes,
and attachments remain intact.

Images that have not downloaded cannot be included. Run `aven sync` first when
the sync server may have files this device lacks. Restore checks the archive and
images before replacing local data.

:::caution[Local data replacement]
Restore replaces local data and requires confirmation with `--yes`. Aven creates
a safety backup first and preserves the previous attachment directory.
:::

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
nonrecurring data. Import keeps this installation's client identity and clears
server-specific sync state.

:::caution[Local data replacement]
Import replaces local data and requires confirmation with `--yes`. Aven creates
a safety backup first.
:::

## Restore attachment images

After importing JSON, attachment labels remain visible while the TUI shows
unavailable-image placeholders. Run sync or restore a backup archive to supply
the files. Until then, [`attachment get --output`](/command-reference/#aven-attachment)
cannot save them.

A complete backup includes only images available locally when it is created. If
you synchronize attachments, confirm that sync is complete before creating the
archive. See [Image attachments during sync](/sync/#image-attachments-during-sync).

See [Data safety commands](/command-reference/#data-safety-commands) for every
backup, restore, export, and import option.
