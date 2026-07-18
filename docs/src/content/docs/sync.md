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

When tasks have image attachments, metadata travels in the operation log while blob bytes use authenticated attachment endpoints. A sync round transfers at most 16 unique blobs and 64 MiB across uploads and downloads. A round that has completed no transfer may take one otherwise valid blob beyond its remaining byte budget so large backlogs keep progressing.

Sync output includes uploaded and downloaded blob counts and byte totals, remaining upload and download counts and bytes, the cursor, and completion state. `complete=false` means metadata or required blob transfers remain. Remaining uploads can be prerequisites for unsynced attachment-add operations, while downloads are limited to attachments on live tasks. Interactive sync runs additional bounded rounds while progress is possible. Duplicate references to one content hash transfer one physical blob.

Local add and download capacity and server workspace upload capacity enforce attachment quotas with `error attachment-quota-exceeded`. Sync exits before printing a completion summary on this error. Pulled metadata and cursor progress remain committed when a local download is blocked. Increase the relevant quota or free eligible original objects, then retry.

:::note[Attachment ordering]
Aven confirms attachment bytes and workspace quota capacity on the server before pushing dependent attachment metadata. Pulled metadata and cursor progress commit before local blob downloads, so a failed download remains pending for a later round without replaying applied metadata.
:::

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

Use compressed backup archives when attachment originals must travel with the database. Use JSON export and import for metadata-only portability.

```sh
aven backup --output backup.aven-backup.tar.zst
aven backup restore backup.aven-backup.tar.zst --yes
```

```sh
aven export --output tasks.json
aven import tasks.json --yes
```

- **Backup and restore:** Backup archives contain a consistent SQLite database, a manifest, and every locally available original attachment object. The database retains all inventory rows, including unavailable rows, while the manifest and object payload contain only locally available inventory. Staging, preview cache, trash, and incomplete files are excluded. Restore validates archive structure, hashes, image content, and attachment metadata before replacing the database and original-object set.
- **Export and import:** JSON includes tasks, workspaces, projects, labels, notes, dependencies, epics, attachment metadata, blob inventory, changes, field versions, conflicts, and portable metadata. It sets `blobs_included: false`. Import marks every blob inventory row unavailable, and those missing bytes do not create upload obligations. Restore or sync can supply bytes later.

:::caution[Local data replacement]
Restore and import replace local data, require confirmation with `--yes`, and create safety backups before replacement. Archive restore also preserves the prior blob directory as a safety copy.
:::

- **Sync metadata:** Import preserves the target client id, resets the sync cursor, and skips imported pinned server metadata.

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

For sync specifically, doctor reports the configured server, sync cursor, pending changes, conflicts, daemon wake validity, and integrity status when requested.
