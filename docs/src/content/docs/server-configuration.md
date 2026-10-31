---
title: Server configuration
description: Configure the self-hosted sync server with server.yaml.
---

The [sync server](/sync/setup/#start-a-server) reads its settings from an
optional `server.yaml`. Without the file, the server uses built-in defaults.
The server never reads the client [`config.yaml`](/configuration/).

Upgrading a server that used `config.yaml`? Follow the
[migration guide](/migration-guide/#move-server-settings-to-serveryaml).

## Where server.yaml lives

`server.yaml` sits in the directory that holds the server storage:

| Deployment | Server storage | `server.yaml` |
| --- | --- | --- |
| Bare process | `~/.local/state/aven/server/sync-server.sqlite` | `~/.local/state/aven/server/server.yaml` |
| systemd with `StateDirectory=` | `$STATE_DIRECTORY/sync-server.sqlite` | `$STATE_DIRECTORY/server.yaml` |
| Docker or Compose | `/data/sync-server.sqlite` | `/data/server.yaml`, on the data volume |

With `--data PATH`, the file is `server.yaml` beside `PATH`. To keep it
elsewhere, pass `--config PATH` or set `AVEN_SERVER_CONFIG`. The flag takes
precedence over the environment variable. The server logs the path it uses at
startup.

`aven server setup` writes a commented template when the file does not exist.
Run setup as the user the server runs as, with the same `--data` or `--config`.
A systemd unit with `StateDirectory=` sets `STATE_DIRECTORY` only for the
service, so pass `--data` from a shell.

## Settings

Every setting is optional:

```yaml
attachments:
  grace_days: 30
  workspace_quota_bytes: 10737418240
```

| Setting | Default | Purpose |
| --- | --- | --- |
| `attachments.grace_days` | 30 | Minimum age before an unused server image becomes eligible for cleanup. |
| `attachments.workspace_quota_bytes` | 10 GiB | Maximum unique image storage for one server workspace. |

## Restart after every change

The server reads `server.yaml` at startup. Restart it after changing the file:

| Deployment | Restart |
| --- | --- |
| Bare process | Stop `aven server` and start it again with the same options. |
| systemd user unit | `systemctl --user restart aven-server` |
| Compose | `docker compose restart aven` |
| Plain Docker | `docker restart aven-server` |

## Invalid configuration stops the server

An unknown key, a value out of range, or a file that does not parse stops the
server at startup. The error identifies the file and the invalid setting.
