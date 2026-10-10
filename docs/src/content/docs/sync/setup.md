---
title: Sync across devices
description: Synchronize Aven data with end-to-end encryption, resolve conflicts, and diagnose sync state.
---

Sync keeps the same aven tasks available across laptops, agents, and other devices. Each client writes to its own local SQLite database first, so task capture and updates stay fast and work offline.

Sync is end-to-end encrypted. Devices encrypt tasks, history, and images before
upload, so the self-hosted server cannot read your tasks or images. One
device starts the sync from its database, and each other device joins with an
invitation from a device that already syncs.

The sync server is not a backup. Only your devices hold the decryption keys, so
if every device is lost, the server cannot restore your data. Keep regular
[backups](/backups/). See [what encryption protects](#what-encryption-protects)
for what the server can still see.

## Start a server

The sync server is a single `aven server` process that you host yourself on a
machine all your devices can reach. The recommended setup is a private network
such as Tailscale or WireGuard: devices connect to the server's VPN address,
and nothing is exposed to the internet. To reach the server over the public
internet instead, put it behind a TLS reverse proxy and use its HTTPS URL.

Prepare server storage once, giving the URL devices will use to reach it, then
serve it:

```sh
aven server setup --url http://100.100.20.30:3746
aven server --bind 100.100.20.30:3746
```

Storage goes in `~/.local/state/aven/server/sync-server.sqlite`, or in the
directory systemd provides when the service declares `StateDirectory=`. Setup
prints the path it used. To keep it elsewhere, pass the same `--data <path>` to
both commands.

`server setup` prints a setup invitation for the next step. Anyone with it can
claim the server, so use it only on the device whose data should start the
sync. It expires after one hour; run `server setup` again for a new one.

See [`aven server`](/command-reference/#aven-server) for URL rules and bind
options.

### Use a public TLS proxy

The server does not terminate TLS. Give `server setup` the public HTTPS origin
with no path prefix, then route that origin to the server's loopback address.
Anyone can send requests to a public server and keep it busy, so prefer a VPN;
if you do expose it, rate-limit at the proxy. A Caddy proxy needs only the
upstream:

```text
sync.example.com {
    reverse_proxy 127.0.0.1:3746
}
```

For nginx, allow encrypted image chunks and bootstrap batches up to 4 MiB plus
framing. Keep send and read
timeouts longer than Aven's 35-second request deadline; the connection timeout
can stay shorter:

```nginx
location / {
    client_max_body_size 8m;
    proxy_connect_timeout 10s;
    proxy_send_timeout 40s;
    proxy_read_timeout 40s;
    proxy_pass http://127.0.0.1:3746;
}
```

Default Caddy body and timeout settings need no changes.

### Run with Docker

The image is the aven CLI: with no arguments it starts the sync server.
Write commands exactly as on a native install, including `server setup`.
The image runs only the sync server; run `aven sync …` and task commands on
your devices, not inside the container.

The container runs as a non-root user and stores the server database in
`/data/sync-server.sqlite`. Mount a named volume at `/data` for both setup and
serving.

Build the checked-out source locally:

```sh
docker build -t aven:local .
export AVEN_IMAGE=aven:local
```

Stable release images are available at `ghcr.io/raine/aven` for Linux amd64
and arm64. Select an exact version (without the `v` prefix):

```sh
export AVEN_IMAGE=ghcr.io/raine/aven:VERSION
```

Replace `VERSION` with the release number. `latest` tracks the current stable
release, but pin a version or digest for deliberate upgrades.

#### Compose behind an HTTPS proxy

Use the repository's [`compose.yaml`](https://github.com/raine/aven/blob/main/compose.yaml),
or save this as `compose.yaml`:

```yaml
name: aven-sync
services:
  aven:
    image: ${AVEN_IMAGE:?Set AVEN_IMAGE to a versioned ghcr.io/raine/aven image or a local build}
    ports:
      - "${AVEN_HOST_IP:-127.0.0.1}:3746:3746"
    volumes:
      - aven-data:/data
    restart: unless-stopped
    stop_grace_period: 15s
volumes:
  aven-data:
```

Configure your externally managed HTTPS proxy on the host to forward to
`127.0.0.1:3746`, as in [Use a public TLS proxy](#use-a-public-tls-proxy).
Prepare storage with the HTTPS origin devices will use, without a path prefix:

```sh
docker compose run --rm --no-deps aven server setup --url https://sync.example.com --invitation-only
docker compose up -d
docker compose logs -f --timestamps --since 1m aven
```

Run setup in a one-off container that shares the service's volume, as shown.
If the service started first, it restarts until setup succeeds and can't be
reached with `docker exec`; the one-off container works either way.

Use timestamps when checking logs: `server-storage-unprepared` errors from
before setup are expected. If the service still reports unprepared storage
after setup, setup and the service are using different volumes or directories.

The setup command prints only the private `aven-setup:` invitation. Use it on
the starting device as in [Set up sync from one device](#set-up-sync-from-one-device).
It expires after one hour; before the server is claimed, explicitly run setup
again if you need a replacement.

Keep the Compose project name and volume when replacing containers. The volume
is `aven-sync_aven-data` with the example's project name. `docker compose down`
keeps it; **`docker compose down --volumes` deletes server storage**.

Keep the published port bound to `127.0.0.1` so only the host's HTTPS proxy can
reach it. If your proxy runs in another container, connect them over a private
Docker network instead of publishing a public plaintext port.

For a trusted VPN instead of HTTPS, publish only on the host's VPN address and
use that origin for setup:

```sh
export AVEN_HOST_IP=100.100.20.30
docker compose run --rm --no-deps aven server setup --url http://100.100.20.30:3746 --invitation-only
docker compose up -d
```

The VPN address must be available when Docker starts the service.

#### Plain Docker commands

Use the same volume for setup and serving. This example assumes the host HTTPS
proxy above:

```sh
docker volume create aven-data
docker run --rm -v aven-data:/data "$AVEN_IMAGE" server setup --url https://sync.example.com --invitation-only
docker run -d --name aven-server --restart unless-stopped --stop-timeout 15 \
  -v aven-data:/data -p 127.0.0.1:3746:3746 "$AVEN_IMAGE"
```

Check startup with `docker logs -t aven-server`. Errors from before setup are
expected; if unprepared-storage errors continue, check that setup and the
service share the same storage.

Arguments replace the entire default command. To override serving options,
include `server` and all the serve flags you need:

```sh
docker run -d --name aven-server --restart unless-stopped --stop-timeout 15 \
  -v aven-data:/data -p 127.0.0.1:3746:3746 "$AVEN_IMAGE" \
  server --bind 0.0.0.0:3746 --unsafe-public-bind --data /data/sync-server.sqlite
```

For direct VPN access, replace `127.0.0.1` in the published port with the host's
VPN address and use that address in the setup URL.

For a host bind mount instead of a named volume, give the directory UID/GID
`65532:65532` and owner-only access before mounting it at `/data`. Keep the
service running as non-root.

#### Upgrade and preserve storage

Read release notes for server and device compatibility before upgrading. Stop
the service before backing up storage:

```sh
docker compose stop
```

Back up the **entire volume**, including SQLite sidecar files, while the
service is stopped, and keep the copy private. Upgrades may migrate server
storage, so do not roll back by simply running an older image. Server-volume
backups do not replace [client backups](/backups/): encryption keys live on
devices.

Then select the new exact image version, pull it, and recreate the service:

```sh
export AVEN_IMAGE=ghcr.io/raine/aven:NEW_VERSION
docker compose pull
docker compose up -d
```

For a local build, build the new source with a new tag and select that tag
instead of pulling. For plain Docker, stop and remove only `aven-server`,
preserve `aven-data`, then recreate the container with the new image and the
same volume/port settings. Do not rerun setup during an upgrade.

#### Start over with new server storage

1. Stop and remove the service container, keeping its storage: use
   `docker compose down` **without `--volumes`**, or
   `docker stop aven-server && docker rm aven-server`. An existing container
   keeps its mounts, so `docker start` would reuse the old storage.
2. Keep the old volume or directory until the new sync works.
3. Create new storage. For plain Docker, run `docker volume create aven-data-2`.
   For Compose, choose a new volume name in both the service's `volumes:` entry
   and the top-level `volumes:` declaration. For a bind mount, use a new empty
   directory owned by `65532:65532` with mode `700`.
4. Run `server setup` in a one-off container on the new storage, using the setup
   command above with the new volume or directory.
5. Recreate the service: `docker compose up -d`, or the same plain-Docker
   `docker run` command above with the new `-v`, keeping port and restart
   settings.
6. If a device's setup never finished, run `aven sync reset --force`, then
   `aven sync setup` with the new invitation; see the
   [interrupted-setup guidance](#set-up-sync-from-one-device). For an established
   sync, follow [Rebuilding sync](#rebuilding-sync), using plain `aven sync reset`.

### Run the server as a service

Once sync works, run `aven server` under your operating system's service
manager so it starts at boot. On Linux, a systemd user service works; save this
as `~/.config/systemd/user/aven-server.service`, adjusting the binary path and
bind address:

```ini
[Unit]
Description=Aven sync server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/aven server --bind 100.100.20.30:3746
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

Then enable it, and enable lingering so it keeps running after you log out:

```sh
systemctl --user daemon-reload
systemctl --user enable --now aven-server
loginctl enable-linger
journalctl --user -u aven-server -f
```

Binding to a VPN address such as a Tailscale IP requires the VPN interface to
be up; if the server fails at boot, systemd retries it every five seconds.

## Set up sync from one device

On the device whose data should start the sync, copy the setup invitation and
run:

```sh
aven sync setup
```

On macOS, interactive setup automatically uses a valid `aven-setup:` invitation
from the clipboard. Otherwise, paste the invitation at the prompt. Setup shows
the server and what this database will publish, then asks for confirmation.
Every other device starts from this data.

In the TUI, open the Sync dialog with `:sync`, `C s`, or a click on the sync
indicator in the header, and choose **Set up sync**.

If setup is interrupted, run the same command again, or choose **Resume
setup**, to continue. Resume with an invitation from the same server storage.
If that storage or its invitation is permanently gone, abandon the unfinished
setup while keeping local data, then start again:

```sh
aven sync reset --force
aven sync setup
```

## Add a device

On a device that already syncs, create an invitation and leave the command
running:

```sh
aven sync invite
```

It prints the invitation and shows it as a QR code. In the TUI, choose **Add
device** in the Sync dialog.

:::caution[Keep invitations private]
Anyone with the invitation can read all synced data and manage devices. It
expires after ten minutes. To cancel it early, press Ctrl-C or run
`aven sync invite --cancel`.
:::

On the new device, join from an empty database and paste the invitation:

```sh
aven sync join
```

Confirm the server, and your tasks download, followed by their images. In the
TUI, choose **Join existing sync**. If joining is interrupted, run
`aven sync join` again, or choose **Resume joining**.

A database that already has tasks cannot join, because Aven cannot merge
existing local data into sync. Join with a new database path instead, and use
that path from then on:

```sh
aven --db /path/to/new.sqlite sync join
```

## Automate sync with the daemon

The daemon syncs in the background: after local edits, periodically, and with
retries after failures. Enable automatic sync and install it as a service:

```sh
aven config set sync.enabled true
aven daemon install
```

On macOS this installs a user LaunchAgent; on Linux, a systemd user service.
On Linux, run `loginctl enable-linger` so it keeps running after you log out.
`aven daemon status` shows whether the service is installed and running. See
[`aven daemon`](/command-reference/#aven-daemon) for the other subcommands.

## Sync manually

```sh
aven sync
```

The output lists the changes sent and received and any new conflicts. In the
TUI, press `S`, or choose **Sync now** in the Sync dialog.

### Image attachments during sync

Images sync after their tasks. If sync reports that images are still
transferring, run `aven sync` again or let the daemon finish in the background.

## Check sync status

```sh
aven sync status
```

Status reads local state without contacting the server. It shows the server,
local changes waiting to sync, open conflicts, and pending image transfers. The
Sync dialog in the TUI shows the same overview and whether automatic sync is
on.

If the server refuses this device, status says so. This does not prove the
device was removed; check from another device. Local tasks stay available.

## Manage devices

List the devices in the sync:

```sh
aven sync device list
```

Each device shows its name (the macOS Computer Name or Linux hostname) and a
short ID. Names live in the encrypted data, so the server never sees them.

Remove a device by its ID or a unique prefix of it:

```sh
aven sync device remove 3f9a
```

Removal stops that device from syncing and changes the encryption keys, so it
cannot read changes made after your other devices sync and pick up the new
keys. It keeps whatever it already downloaded. To bring it back, add it again
with a new invitation, as in [Add a device](#add-a-device).

Run removal from another device; Aven does not let a device remove itself. In
the TUI, choose **Manage devices** in the Sync dialog.

To stop syncing this database but keep its data locally, use
[`aven sync reset`](/command-reference/#aven-sync-reset). Reset does not remove
the device from the sync; do that from another device.

A sync allows a limited number of device additions and removals over its
lifetime. When the limit is reached, Aven says so; start a new sync as in
[Recover from device loss](#recover-from-device-loss).

## Resolve conflicts

Conflicts happen when two devices edit the same field of the same task between
syncs. Aven keeps both values and asks you to choose. `conflict show` prints
each value with a token such as `v7CQBAP`; pass the token of the value to keep:

```sh
aven conflict list
aven conflict show APP-7KQ9
aven conflict resolve APP-7KQ9 description --use v7CQBAP
```

See [`aven conflict`](/command-reference/#aven-conflict) for exporting both
versions or resolving with a custom value. In the TUI, press `v c` to review
tasks with conflicts.

## Diagnose sync state

`aven doctor` reports whether the database is set up, pending changes,
conflicts, and daemon configuration. Sync errors explain what went wrong and
the next step, with a stable code in square brackets.

## Recover from device loss

If one device is lost or broken, add a replacement as in
[Add a device](#add-a-device), inviting it from a device that still syncs. No
backup is needed. Then remove the lost device from the sync.

If every syncing device is lost, restore a backup to a fresh database path and
start a new sync from it:

```sh
aven --db /path/to/recovered.sqlite backup restore backup.aven-backup.tar.zst --yes
aven server setup --data /path/to/new-sync-server.sqlite --url http://100.100.20.30:3746
aven server --data /path/to/new-sync-server.sqlite --bind 100.100.20.30:3746
aven --db /path/to/recovered.sqlite sync setup
```

Do not reuse the old sync's server storage. Join every other device to the new
sync from an empty database. Changes made after the backup that never synced
are not included; check any old database you can still access before
discarding it.

A syncing database cannot be restored over or imported into, so always restore
to a new path. See [Back up and restore](/backups/) for backup contents.

## Rebuilding sync

Sync stops on a device when it receives a change it can't apply. If Aven on
that device is out of date, update it. If the change itself is damaged, sync
stops at the same place every time; `aven sync` says which case applies. Local
tasks stay safe and editable.

To keep syncing after a damaged change, start a new sync on new server storage
from the device with the best data:

1. Prepare new server storage and serve it, as in
   [Start a server](#start-a-server). Do not reuse the old storage.
2. On the device with the best data, reset sync and set it up again:

   ```sh
   aven sync reset
   aven sync setup
   ```

3. On each other device, check `aven sync status` for local changes that never
   synced. Preserve its database and create a backup with `aven backup` before
   replacing it; JSON exports do not include image files. Changes unique to
   that device are not merged into the new sync. Then reset and join the new
   sync from a new database path:

   ```sh
   aven sync reset
   aven --db /path/to/new.sqlite sync join
   ```

## What encryption protects

Encryption covers synced content. It does not cover:

- **Your devices.** Databases, images and backups on each device are readable.
  Rely on your operating system's account and disk protection.
- **Metadata.** The server sees device identities, record counts and sizes,
  timing, connection details, the random IDs of tasks, workspaces and images,
  which records belong together, and the exact size of each encrypted image.
  Recurring task IDs derive from the recurrence, so someone who knows most of
  one can confirm a guess.
- **Availability.** A malicious server can withhold data or show devices stale
  or different views.
- **Device trust.** Every paired device can add and remove devices, including
  all the others. Pair only devices you control.
- **Credentials.** Each device stores its sync credential on disk: owner-only
  files on Linux, Keychain-protected files on macOS. Aven backups leave it out,
  but home-directory backups such as Time Machine may copy it. A credential
  cannot decrypt anything, but its holder can upload records that stop other
  devices from syncing or mark records deleted. Use HTTPS or a trusted VPN, and
  remove a lost device promptly.

## Upgrade from unencrypted sync

Earlier releases synced without end-to-end encryption. Encrypted sync can't
use that server storage, so every device moves to a new sync:

1. Before upgrading, run `aven sync` with the old release on every available
   device, then sync the device whose data will start the new sync again to
   receive their changes and images. Check that sync is complete and
   `aven sync status` shows no pending changes or image transfers. Create a
   backup with `aven backup`, and preserve the old server database and image
   storage together. If a device cannot sync, preserve its database and backup;
   its unique edits are not included, so do not erase or replace it.
2. Upgrade Aven everywhere, including the server. Homebrew and the install
   script upgrade normally. On a direct install with sync configured,
   `aven update` refuses because it can't verify sync compatibility and
   suggests updating the server first. Updating the server doesn't help here,
   so after step 1 run `aven update --yes --allow-sync-incompatibility`.
3. Prepare and serve new server storage, as in
   [Start a server](#start-a-server), with a new `--data` path.
4. On the device with the complete data, run `aven sync setup`.
5. Join each other device from a new database path, as in
   [Add a device](#add-a-device).

The new sync carries current images and extra image files available on the
starting device. Deleted images held only by the old server are not transferred.
Keep the old server storage and backups for as long as you need that recovery
option; they still hold your data unencrypted.

Old storage protects the pre-upgrade checkpoint, not edits made after cutover.
Before rolling back, preserve each device's new work and images with a backup.
Aven does not merge changes between the old and new sync.
