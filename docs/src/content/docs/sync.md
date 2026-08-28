---
title: Sync across devices
description: Synchronize Aven data, resolve conflicts, and diagnose sync state.
---

Sync keeps the same aven tasks available across laptops, agents, and other devices. Each client writes to its own local SQLite database first, so task capture and updates stay fast and offline-friendly.

When you run `aven sync`, local changes are pushed to a self-hosted server and changes from other clients are pulled back down. The server stores the shared operation log; each local database applies that log to its own task store.

Use [Configuration](/configuration/) for `sync.*` and `daemon.*` settings. See
[Back up and restore](/backups/) when you need to preserve, move, or recover
local data.

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

Check local health and remaining work without contacting the server:

```sh
aven sync status
aven sync status --json
```

The status report shows whether sync is disabled, unconfigured, healthy,
degraded, blocked, or failed. It also shows server pinning, pending changes and
images, conflicts, cursor progress, and last attempt and success times. JSON is
a versioned report intended for scripts and omits authentication values, task
content, sync payloads, and raw server responses.

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

This separation also affects data portability. [Back up and restore](/backups/)
compares complete backup archives with portable JSON exports.

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

For sync specifically, doctor reports the configured server, sync cursor, pending changes, conflicts, daemon wake validity, and integrity status when requested. A missing current recurring task is repairable by running `aven recur list`. Other recurring-task integrity failures require preserving the database and recovering from a known-good backup.
