---
title: Sync and backups
description: Sync across devices, resolve conflicts, and back up your data.
---

Sync keeps the same aven tasks available across laptops, agents, and other devices. Each client writes to its own local SQLite database first, so task capture and updates stay fast and offline-friendly.

When you run `aven sync`, local changes are pushed to a self-hosted server and changes from other clients are pulled back down. The server stores the shared operation log; each local database applies that log to its own task store.

Use [Configuration](/configuration/) for `sync.*` and `daemon.*` settings.

## Start a server

```sh
aven server --bind 127.0.0.1:0 --data /tmp/aven-server.sqlite
```

The server stores sync data in the SQLite file passed with `--data` and prints a listening URL:

```txt
listening url=http://127.0.0.1:<port> scope=loopback
```

Network requirements depend on the bind address:

| Scope | Use | Requirements |
| --- | --- | --- |
| Loopback | Local testing | Authentication optional |
| Private address | LAN, VPN, or another private network | `sync.auth_token` on the server and matching client tokens |
| Public address | Internet-facing service | `--unsafe-public-bind`, `sync.auth_token`, and TLS or a reverse proxy |

:::caution[Network security]
Plain HTTP is intended for loopback, trusted VPNs, private networks, or external TLS termination. Sync server URLs must use `http` or `https`, include a host, and omit username, password, query, and fragment parts.
:::

## Set up sync over a VPN

Aven does not create or manage the VPN. The server and every client must already be connected to the same private network. This example uses `10.0.0.1` as the server's VPN address.

First, generate a shared authentication token:

```sh
openssl rand -hex 32
```

Store the generated value in `~/.config/aven/config.yaml` on the server:

```yaml
sync:
  auth_token: "<generated-token>"
```

Start the server on its VPN address, not its loopback address:

```sh
mkdir -p ~/.local/state/aven
aven server \
  --bind 10.0.0.1:3746 \
  --data ~/.local/state/aven/sync-server.sqlite
```

Run this command under your operating system's service manager after confirming that sync works. The service must start after the VPN interface is available.

On each client, store the same token and use the server's VPN address:

```yaml
sync:
  enabled: true
  server_url: "http://10.0.0.1:3746"
  auth_token: "<generated-token>"
  interval_seconds: 30
```

Verify the network path before testing Aven:

```sh
ping 10.0.0.1
nc -vz 10.0.0.1 3746
aven sync
```

## Sync a client

```sh
aven sync --server http://127.0.0.1:<port>
```

When `sync.server_url` is configured, `aven sync` can omit `--server`:

```sh
aven sync
```

### Image attachments during sync

#### How attachment sync works

Aven syncs an attachment record and its image file as related but separate data. The record identifies the task, media type, dimensions, and other attachment information. The image file is stored and transferred separately. This keeps normal task changes responsive even when images are large or a device is temporarily offline.

The cross-device flow is:

1. The task and attachment record sync through the normal change stream.
2. The image uploads to the sync server in a separate, bounded transfer.
3. Another device can receive the task and attachment record before it receives the image. That device shows `pending download` until a later sync transfers the file.
4. Each device tracks its own local image availability and storage quota. The server enforces a separate quota for each workspace.
5. Images are addressed by their content, so identical image bytes share storage and transfer identity instead of creating independent copies.
6. Deleting an attachment syncs that change. Image bytes with no remaining references become eligible for cleanup after the configured grace period rather than disappearing immediately.

This separation also affects data portability. The [backup and export comparison](#back-up-and-move-data) explains the available options. A backup archive includes every attachment image available on the device that creates it, while a JSON export includes attachment records but omits image files.

Sync output reports image upload and download counts, transferred sizes, remaining work, and completion state. Run `aven sync` again when it reports `complete=false`. Interactive sync continues automatically while it can make progress. A configured, running daemon resumes remaining work in the background.

`error attachment-quota-exceeded` can refer to storage on this device or to the server workspace. For local storage, remove eligible images with `aven attachment prune` or increase [`quota_bytes`](/configuration/#retention-and-storage-limits). For server storage, increase `server_workspace_quota_bytes` in the server's configuration. Local pruning does not reduce server workspace usage. Task changes already downloaded remain available, and missing images can download on a later sync.

:::note[Server pinning]
A local database pins the sync server it has used. Use a fresh database for a different server.
:::

## Automate sync with the daemon

The daemon performs background sync for the configured local SQLite database.

```sh
aven daemon
```

Daemon sync requires `sync.enabled = true` and `sync.server_url`. The wake address must be loopback.

The daemon wakes after successful local mutations when possible, syncs periodically, reschedules incomplete sync quickly, and backs off after failures.

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

Choose the command that matches what you need to preserve:

| Goal | Commands | Image files included? |
| --- | --- | --- |
| Preserve or move all local Aven data | `aven backup` and `aven backup restore` | Yes, for images available on this device when the backup is created |
| Move task data through JSON | `aven export` and `aven import` | No |

```sh
aven backup --output backup.aven-backup.tar.zst
aven backup restore backup.aven-backup.tar.zst --yes
```

A backup archive contains the complete SQLite database and every attachment image available on the device. Recurring schedules, future-task settings, completed, skipped, and missed history, pauses, conflicts, and task-specific fields, notes, and attachments remain intact. Images that have not downloaded cannot be included, so run `aven sync` first when the sync server may have files this device lacks. Restore checks the archive and images before replacing local data.

```sh
aven export --output tasks.json
aven import tasks.json --yes
```

A JSON export includes tasks, recurring schedules, future-task settings, history, pauses, conflicts, and attachment information. It sets `blobs_included: false` and leaves out image files. Import validates recurring schedules, generated tasks, history, and lifecycle state before replacement. Older task-only exports import every task as nonrecurring data. After import, attachment labels remain visible while the TUI shows unavailable-image placeholders. Run sync or restore a backup archive to supply the files. Until then, [`attachment get --output`](/command-reference/#aven-attachment) cannot save them.

:::caution[Local data replacement]
Restore and import replace local data and require confirmation with `--yes`. Aven creates safety backups first. Archive restore also preserves the previous attachment directory.
:::

Import keeps this installation's client identity and clears server-specific sync state.

### UniFFI consumers

Swift and other UniFFI hosts choose the durable SQLite path and run synchronous facade calls on one serial Rust worker. Rust owns database migrations, recurring-task storage, sync payload validation, and conflict behavior. Hosts pass prepared sync bodies and responses as opaque bytes and do not serialize recurring-task data themselves. Portable JSON and archive backup workflows remain core and CLI data-safety surfaces. A host-provided backup must coordinate with the Rust worker and use a consistent SQLite backup instead of copying an open database file.

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

For sync specifically, doctor reports the configured server, sync cursor, pending changes, conflicts, daemon wake validity, and integrity status when requested. A missing current recurring task is repairable by running `aven recur list`. Other recurring-task integrity failures require preserving the database and recovering from a known-good backup.
