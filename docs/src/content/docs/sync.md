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

## Pair a mobile device

Configure sync with a server your iPhone can reach and a shared `sync.auth_token`.
If using a VPN, connect both devices first.

On your desktop, run:

```sh
aven sync pair
```

Scan the QR code during Aven iOS onboarding. In the TUI, you can also open the
command panel with `:` and choose `:pair-mobile`.

To pair without a camera, run `aven sync pair --copy` on your local desktop,
transfer the clipboard to your iPhone, and tap **Paste** during onboarding.

If your desktop's configured server address is not reachable from your iPhone,
override it for the invitation:

```sh
aven sync pair --server http://10.0.0.1:3746
```

Treat the QR code and copied invitation like a password: both contain your
shared sync token.

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

Images sync automatically but may arrive after their tasks. If an image shows
`pending download` or sync reports `complete=false`, run `aven sync` again or
let the running daemon finish in the background.

If sync reports `attachment-quota-exceeded`, increase the relevant
[storage limit](/configuration/#retention-and-storage-limits): `quota_bytes`
on the device or `server_workspace_quota_bytes` on the server.

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
