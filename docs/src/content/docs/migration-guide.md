---
title: Migration guide
description: Migration steps for breaking changes in Aven.
---

This guide collects migration steps for breaking changes in Aven. Follow the
sections relevant to your setup when upgrading.

## Unreleased

### Move server settings to server.yaml

`aven server` no longer reads the client `config.yaml`. Its settings belong in
an optional `server.yaml` beside the server's storage. See
[Server configuration](/server-configuration/#where-serveryaml-lives) for the
path for your deployment and how to choose a different location.

If you used the default server image retention and quota, no configuration
change is needed. Without `server.yaml`, the server keeps the defaults: a
30-day grace period for unused images and a 10 GiB image quota per workspace.

If you customized either setting, move your values before restarting the
upgraded server:

1. Create `server.yaml` in the server's storage directory, or edit it if it
   already exists.
2. Copy your values from `config.yaml`, using the new keys:

   | Old key in `config.yaml` | New key in `server.yaml` |
   | --- | --- |
   | `local.attachment_lifecycle.server_grace_days` | `attachments.grace_days` |
   | `local.attachment_lifecycle.server_workspace_quota_bytes` | `attachments.workspace_quota_bytes` |

   For example, a 14-day grace period and a 5 GiB quota become:

   ```yaml
   attachments:
     grace_days: 14
     workspace_quota_bytes: 5368709120
   ```

3. Remove the two old server keys from `config.yaml`; they are ignored. Keep
   your other client settings there. Do not copy the whole client configuration
   into `server.yaml`, which rejects unknown keys.
4. [Restart the server](/server-configuration/#restart-after-every-change).
   Check its startup log for the configuration path and any errors.
