---
title: Server configuration
description: Configure the self-hosted sync server with server.yaml, and set up AI for Ask Aven.
---

The [sync server](/sync/setup/#start-a-server) reads its settings from an
optional `server.yaml`. Without the file, the server uses built-in defaults and
has no AI provider. The server never reads the client
[`config.yaml`](/configuration/).

## Where server.yaml lives

`server.yaml` sits in the directory that holds the server storage:

| Deployment | Server storage | `server.yaml` |
| --- | --- | --- |
| Bare process | `~/.local/state/aven/server/sync-server.sqlite` | `~/.local/state/aven/server/server.yaml` |
| systemd with `StateDirectory=` | `$STATE_DIRECTORY/sync-server.sqlite` | `$STATE_DIRECTORY/server.yaml` |
| Docker or Compose | `/data/sync-server.sqlite` | `/data/server.yaml`, on the data volume |

With `--data PATH`, the file is `server.yaml` beside `PATH`. To keep it
elsewhere, pass `--config PATH` or set `AVEN_SERVER_CONFIG`. The server logs
the path it uses at startup, and `aven server ai status` prints it.

`aven server setup` writes a commented template when the file does not exist.

Run `aven server setup` and the `aven server ai` commands as the user the
server runs as, with the same `--data` or `--config` as the server, so they read
the same file and store logins beside it. A systemd unit with `StateDirectory=`
sets `STATE_DIRECTORY` only for the service, so pass `--data` from a shell. In
Docker, run them inside the service container with `docker compose exec`; it
shares the container's volume and environment.

## Settings

Every setting is optional:

```yaml
ai:
  provider: openai
  model: gpt-4.1-mini
  endpoint: "http://127.0.0.1:11434"
  timeout_seconds: 45
  max_output_tokens: 2048
  max_concurrent_requests: 2
  requests_per_minute: 60
attachments:
  grace_days: 30
  workspace_quota_bytes: 10737418240
```

| Setting | Default | Purpose |
| --- | --- | --- |
| `ai.provider` | none | AI provider, as listed by `aven server ai providers`. Set together with `ai.model`. |
| `ai.model` | none | Model name at the provider. |
| `ai.endpoint` | provider default | URL of an Ollama, llamafile, or OpenAI-compatible server, as the Aven server reaches it. Only for the `ollama`, `llamafile`, and `openai` providers. |
| `ai.timeout_seconds` | 45 | Time limit for one provider call, 1 to 300. |
| `ai.max_output_tokens` | 2048 | Output token limit for one call, 1 to 16384. |
| `ai.max_concurrent_requests` | 2 | Provider calls in progress at once, 1 to 32. |
| `ai.requests_per_minute` | 60 | Provider calls per minute, 1 to 600. |
| `attachments.grace_days` | 30 | Minimum age before an unused server image becomes eligible for cleanup. |
| `attachments.workspace_quota_bytes` | 10 GiB | Maximum unique image storage for one server workspace. |

## Precedence

When the same setting comes from more than one place, the first one wins:

1. Command-line flags. `--config` chooses the file over `AVEN_SERVER_CONFIG`,
   and serving options such as `--bind` exist only as flags.
2. `AVEN_AI_PROVIDER`, `AVEN_AI_MODEL`, and `AVEN_AI_ENDPOINT`, which override
   `ai.provider`, `ai.model`, and `ai.endpoint`. Empty values are ignored.
3. `server.yaml`.
4. Built-in defaults.

The result must still pair a provider with a model. `aven server ai status`
shows whether each value came from `server.yaml` or an environment variable.

## Keep secrets in the environment

`server.yaml` holds no secrets. API-key providers read their key from the
server process environment, in the variable that `aven server ai providers`
lists, such as `OPENAI_API_KEY`. Azure also reads `AZURE_ENDPOINT` and
`AZURE_API_VERSION`. Put the key in an owner-only file that the service manager
loads: `EnvironmentFile=` for systemd, `env_file:` for Compose.

ChatGPT and GitHub Copilot use an OAuth login instead. `aven server ai login`
stores it in `private/ai/` beside `server.yaml`, with owner-only permissions. In
Docker that is `/data/private/ai/`, so the login survives container
recreation. Treat the directory, and server-volume backups that include it,
like a password.

## Restart after every change

The server reads `server.yaml`, its environment, and stored logins once, at
startup. Restart it after any change to them:

| Deployment | Restart |
| --- | --- |
| Bare process | Stop `aven server` and start it again with the same options. |
| systemd user unit | `systemctl --user restart aven-server` |
| Compose | `docker compose restart aven` |
| Plain Docker | `docker restart aven-server` |

A restart keeps the container's environment. After changing an `env_file:` or
`-e` value, recreate the container with `docker compose up -d`, or remove and
rerun it with plain Docker.

## Invalid configuration stops the server

An unknown key, a value out of range, a provider without a model, or a file that
does not parse stops the server at startup. The error names the file, the line,
and the key:

```text
Error: could not parse /data/server.yaml: error: line 2 column 3: unknown field `provder`, expected one of provider, model, endpoint, timeout_seconds, max_output_tokens, max_concurrent_requests, requests_per_minute
```

Missing AI credentials are not a configuration error: the server starts, serves
sync, and reports AI as `authentication_required`.

## AI for Ask Aven

[Ask Aven](/mobile-app/#ask-aven) on iPhone sends requests through your sync
server to an AI provider that you configure on the server. The server holds the
provider credentials; phones need only their sync pairing. AI is optional, and
sync works without it. Ask Aven isn't available on Aven Cloud yet.

Ask Aven needs a model that can call tools. `aven server ai providers` marks
each provider with `tools=true` or `tools=false`. ChatGPT with `gpt-6-astra`
and the OpenAI API with `gpt-4.1-mini` are tested; other tool-capable providers
are allowed but untested.

### Set up with the wizard

1. On the server host, run the wizard:

   ```sh
   aven server ai setup
   ```

   It offers tool-capable providers, tested ones first, then asks for a model
   and, for Ollama, llamafile, or another OpenAI-compatible server, the
   endpoint URL. ChatGPT and GitHub Copilot log in inline. For an API-key
   provider it explains where the server reads the key. Setup writes
   `ai.provider`, `ai.model`, and `ai.endpoint` to `server.yaml`, keeping the
   rest of the file, and warns when an `AVEN_AI_*` variable overrides them. It
   then runs `aven server ai check`, which makes two billable requests, unless
   the API key is missing from the current shell.

2. Restart the server. Setup prints the restart command for systemd, a
   container, or a bare process.

3. Read the AI line in the server log:

   ```text
   ai status=ready provider=openai model=gpt-4.1-mini
   ```

   When AI is not ready, the line says why and what to do next:

   ```text
   ai status=authentication_required provider=chatgpt model=gpt-6-astra hint="run `aven server ai login chatgpt` on the server host, then restart the server"
   ```

   The status is `ready`, `unconfigured`, `authentication_required`, or
   `temporarily_unavailable`.

4. Verify from a computer paired with the server:

   ```sh
   aven ai status
   aven ai check
   ```

   `aven ai status` asks the running server, with that device's sync
   credential and through any reverse proxy, the same way the iPhone does. It
   makes no billable request. `aven ai check` sends the same two billable
   requests as the server-side check, through the running server. `aven doctor`
   also shows a server AI line.

To script setup, pass the choices as flags. Without a terminal, setup requires
`--provider` and `--model`:

```sh
aven server ai setup --provider openai --model gpt-4.1-mini --yes
aven server ai setup --provider ollama --model qwen3 --endpoint http://127.0.0.1:11434 --no-check
```

`--yes` skips confirmation and keeps an existing login. `--no-check` skips the
billable check.

`aven server ai status` and `aven server ai check` read `server.yaml` and the
environment of the shell that runs them, not the running server. Use them to
test a change before restarting, and `aven ai status` to see what the running
server does.

### Quick start: systemd

For the [user unit](/sync/setup/#run-the-server-as-a-service) with an API-key
provider, put the key in an owner-only file:

```sh
install -m 600 /dev/null ~/.config/aven/server.env
echo 'OPENAI_API_KEY=sk-...' >> ~/.config/aven/server.env
```

Load it in the `[Service]` section of `aven-server.service`:

```ini
EnvironmentFile=%h/.config/aven/server.env
```

Run setup with the key in the shell so its check can run, then restart and read
the log:

```sh
(set -a; . ~/.config/aven/server.env; aven server ai setup)
systemctl --user daemon-reload
systemctl --user restart aven-server
journalctl --user -u aven-server -b | grep 'ai status='
```

For ChatGPT or GitHub Copilot, skip the environment file: `aven server ai setup`
logs in inline.

### Quick start: Docker Compose

For an API-key provider, save the key in `aven.env` beside `compose.yaml`,
readable only by you, and uncomment `env_file:` in the
[Compose file](/sync/setup/#compose-behind-an-https-proxy):

```yaml
services:
  aven:
    env_file: aven.env
```

Recreate the container so it gets the key, run the wizard inside it, then
restart and read the log:

```sh
docker compose up -d
docker compose exec aven aven server ai setup
docker compose restart aven
docker compose logs --since 1m aven | grep 'ai status='
```

The wizard writes `/data/server.yaml` and stores ChatGPT or GitHub Copilot
logins in `/data/private/ai/`, so both survive `docker compose up -d` and image
upgrades. With plain Docker, use `docker exec -it aven-server aven server ai
setup`.

### Quick start: Ollama

Ollama needs no API key. Pull a model that supports tools, then point the
server at Ollama:

```sh
ollama pull qwen3
aven server ai setup --provider ollama --model qwen3 --endpoint http://127.0.0.1:11434
```

When the Aven server runs in Docker, `127.0.0.1` is the container itself, not
the host. Use an address the container can reach:

- **Ollama in the same Compose project.** Add an `ollama` service and use
  `--endpoint http://ollama:11434`.
- **Ollama on the host.** Add `extra_hosts: ["host.docker.internal:host-gateway"]`
  to the `aven` service and use `--endpoint http://host.docker.internal:11434`.
  Ollama listens only on `127.0.0.1` by default; set `OLLAMA_HOST` so it also
  listens on the Docker bridge address, and keep that port off the internet.

Run the wizard with `docker compose exec` so its check connects from inside the
container, as the server does.

### Log in and out

ChatGPT and GitHub Copilot use OAuth. The wizard logs in when needed; to log in
again or switch accounts, run on the server host:

```sh
aven server ai login chatgpt
aven server ai logout chatgpt
```

Login stores credentials but does not select the provider. Restart the server
after either command. `logout` removes the stored credentials from the server;
revoke the authorization in the provider's account settings if you also need
that.

### Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| The log or `aven ai status` still shows the old state | The server hasn't restarted since the change. Restart it. |
| `aven server ai status` is `ready`, but the running server reports `authentication_required` | The API key is in your shell but not in the server's environment. Add it to `EnvironmentFile=` or `env_file:`, then restart, or recreate the container. |
| `aven server ai status` shows a different config path than the server log | The command used another user, `--data`, or `--config`. Run it like the server does. |
| `unconfigured` with a provider set | The provider name is not one listed by `aven server ai providers`. |
| `aven ai status` reports `route_missing` (HTTP 404) | The server predates AI support, or a reverse proxy doesn't forward `/ai/` paths. |
| `aven ai status` reports `unauthorized` (HTTP 401) | The server refused this device's credential. Check `aven sync status`; the device may have been removed. |
| `tools_unsupported`, or the check fails at the tool call | The provider or model can't call tools. Choose a provider with `tools=true` and a model that supports tools. |
| The check fails with `timeout`, or the proxy returns 504 | The model is too slow for `ai.timeout_seconds`, or the proxy's read timeout is shorter. Keep the proxy's timeout above `ai.timeout_seconds`, as in [the nginx example](/sync/setup/#use-a-public-tls-proxy). |
| Ollama connection refused from Docker | `127.0.0.1` is the container. See [Quick start: Ollama](#quick-start-ollama). |
| Requests fail with `rate_limit` | Devices exceeded `ai.requests_per_minute` or `ai.max_concurrent_requests`, or the provider rate-limited the account. |
| The server exits at startup after an edit | `server.yaml` is invalid. The error names the line and key. |

### Limits and billing

Each Ask Aven round is one provider call. All devices share the server's
`ai.requests_per_minute` and `ai.max_concurrent_requests`; a call over either
limit is refused with `rate_limit` rather than queued. The counters reset when
the server restarts. A request body may be up to 512 KiB, and each call is
bounded by `ai.timeout_seconds` and `ai.max_output_tokens`.

Aven passes the output token limit to providers that support it and rejects
responses that exceed it. The ChatGPT subscription doesn't accept the limit, so
an over-limit response is rejected after the provider has generated, and
billed, it.

When a phone disconnects or a call times out, the server stops waiting for the
provider. Some providers keep generating, and billing, after that.

`aven server ai check` and `aven ai check` each make two billable calls.
`aven server ai status`, `aven ai status`, and `aven doctor` make none.

### Voice transcription

When the server's provider is ChatGPT and its login is present, phones can
send voice recordings to the server for transcription instead of transcribing
on the device. Other providers don't offer this. It needs no extra setting.

The server forwards each recording to ChatGPT's transcription service with the
server's ChatGPT login and returns only the text. That service is an internal
ChatGPT endpoint, not a documented API, so it can change or stop working
without notice. Phones record at most five minutes of AAC audio in an M4A file,
up to 4 MiB, and each recording counts against the same `ai.requests_per_minute`,
`ai.max_concurrent_requests`, and `ai.timeout_seconds` as Ask Aven calls.

### Privacy

With on-device dictation, Ask Aven turns speech into text on the phone and
audio never leaves it. With [voice transcription](#voice-transcription), the
recording goes through your server to OpenAI. The server doesn't store the
audio or the text, but OpenAI reports keeping each uploaded recording for 30
days, and its own retention policy applies. The phone sends your request, the conversation so far, and the task content the AI asks
for, such as titles, descriptions, notes, projects, and labels, to your server.
The server forwards them to the configured provider, whose own retention
policy applies. Encrypted sync does not protect this text from the server or
the provider.

The server can't read your synced tasks, and doesn't run tools or keep
conversations: it makes one provider call per request. Its logs record the
provider, outcome, duration, and token counts, never prompts, task content,
responses, audio, or transcripts.

Every device enrolled in sync can use the server's AI, with its sync credential.
Removing a device from sync also removes its AI access, without a restart.
