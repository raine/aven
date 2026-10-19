---
title: AI completion server
description: Configure Aven's generic, self-hosted AI completion service.
---

The completion server lets paired clients use an administrator-configured AI provider without storing provider credentials on each client. It accepts conversation history and tool definitions and returns text, native tool calls, and opaque reasoning. JSON-schema output is also available.

The iOS agent runs its loop on the device: it sends a completion request, executes the returned tools against its local tasks, then sends the results with the conversation history for the next completion. End-to-end encrypted sync keeps task data readable only on the device; the device chooses what context to send to AI. The server does not read the sync database, execute tools, or create or modify Aven data. It performs one provider call per HTTP request and keeps no conversation or turn state.

AI is optional. An unconfigured AI service does not prevent the sync server from starting or serving sync requests.

## Configure the service

Configure AI on the server host, not on client devices. The server reads the `ai` section of `server.yaml` in the directory that holds its storage, such as `/data/server.yaml` in the Docker image, or the file given by `--config` or `AVEN_SERVER_CONFIG`. It does not read the client `config.yaml`. Run the `aven server ai` commands on the server host as the user the server runs as, with the same `--data` or `--config` as the server. See [Server settings](/configuration/#server-settings) for the file location and template.

Add an `ai` section to `server.yaml`:

```yaml
ai:
  provider: ollama
  model: llama3.2
  endpoint: "http://127.0.0.1:11434"
  timeout_seconds: 45
  max_output_tokens: 2048
  max_concurrent_requests: 2
  requests_per_minute: 60
```

AI routes require the same per-device credential used by encrypted sync, including on loopback. A device must be enrolled in the server’s published vault and not revoked. Pair the device through the normal sync enrollment flow; no separate AI token is needed. Every enrolled device can use configured model capacity. Revoking a device also removes its AI access without restarting the server.

`AVEN_AI_PROVIDER`, `AVEN_AI_MODEL`, and `AVEN_AI_ENDPOINT` override the matching file values; `aven server ai status` reports where each value came from. `provider` and `model` must still be set together. An invalid value stops the server from starting. Changes apply after the server restarts.

The server chooses the provider, model, endpoint, timeout, and limits. Completion requests cannot override them. `ai.endpoint` is available for llamafile, Ollama, and custom OpenAI-compatible endpoints. Other providers use their standard endpoints.

## Providers and credentials

List the available providers, credential environment variables, tool support, and JSON-schema strategies:

```sh
aven server ai providers
```

The provider registry includes Anthropic, Azure OpenAI, ChatGPT subscription OAuth, Cohere, GitHub Copilot, DeepSeek, Doubleword, Gemini, Groq, Hugging Face, Hyperbolic, llamafile, MiniMax, Mira, Mistral, Moonshot, Ollama, OpenAI, OpenRouter, Perplexity, Together, Venice, xAI, Xiaomi MiMo, and Z.AI. Voyage AI is excluded because it does not provide text completions.

API-key providers read the environment variable shown by `aven server ai providers` from the server process. Azure also uses `AZURE_ENDPOINT` and `AZURE_API_VERSION`. Keep these values in the service manager's protected secret environment, not in `server.yaml`.

OAuth providers store material in `private/ai/` beside `server.yaml`, for example `/data/private/ai/` in the Docker image, so a container keeps its login on the data volume. Aven sets the private AI directory to owner-only permissions on Unix. It does not include this directory in sync, backups, or exports.

Start an interactive OAuth flow only from an administrator terminal:

```sh
aven server ai login chatgpt
aven server ai login copilot
```

Only ChatGPT and GitHub Copilot use `login`. Without a provider argument or a configured `ai.provider`, `login` lists them. For an API-key provider it explains which environment variable the server reads instead of starting a flow.

Login stores credentials but does not select the server provider or model. Set `ai.provider` to `chatgpt` or `copilot`, set `ai.model` to a model available to the authenticated account, restart the server, then run `aven server ai check`.

The long-running server does not start an interactive login. Missing OAuth material produces `authentication_required`. Expired or invalid credentials fail the provider request.

### Log out locally

Remove locally stored OAuth credentials with:

```sh
aven server ai logout chatgpt
aven server ai logout copilot
```

Restart the server afterwards so it stops using them. This removes Aven's local credentials. It does not revoke access at the provider. Use the provider's account settings when you also need to revoke the authorization.

## Check the setup

At startup the server logs its config path and one AI line, for example:

```text
ai status=authentication_required provider=chatgpt model=gpt-5 hint="run `aven server ai login chatgpt` on the server host, then restart the server"
```

Inspect configuration and credential presence without contacting or billing a provider:

```sh
aven server ai status
aven server ai status --json
```

`status` prints the config path, credential directory, provider and model with where each came from (`server.yaml`, an `AVEN_AI_*` variable, or not set), the availability status, and a hint with the next step. It reads `server.yaml` and the environment of the process running the command, not the running server, so a change shows here before the server picks it up at its next restart.

A `ready` status does not verify that the configured model is available to the account or can call tools. Run the billable check to verify the complete setup:

```sh
aven server ai check
aven server ai check --json
```

The check makes two completions with an inert synthetic tool: the first must return a call to the tool, the second replays that assistant message unchanged with a tool result and must return a text answer. The iOS agent needs both. Output lists each step; a failing step shows its error category and a hint. The command exits non-zero when any step fails and never prints credentials or provider responses. Like `status`, it runs from the current process with the server's configuration, not through the running server.

## HTTP contract

Both routes require `Authorization: Bearer <device-secret>`, where `device-secret` is the enrolled device’s 32-byte sync secret encoded as 64 lowercase hexadecimal characters:

```text
GET  /ai/capabilities
POST /ai/completions
```

Missing, malformed, unknown, and revoked credentials all receive HTTP 401 without distinguishing the reason. Setup invitations and shared tokens do not authorize AI access. Send credentials only over a protected connection and never log them.

Capabilities are local and nonbillable. `completion_contract` is currently `1`. Stable availability states are `ready`, `unconfigured`, `authentication_required`, and `temporarily_unavailable`. An older server without AI routes returns 404.

Completion contract 1 accepts:

- `system` and `user` messages containing `text` blocks
- `assistant` messages containing ordered `text`, `tool_call`, and `reasoning` blocks
- `tool` messages containing `tool_result` blocks with a call ID, tool name, and string output (often JSON)
- optional `tools`, each with `name`, `description`, and a JSON-schema `parameters` object
- `output: {"type": "text"}` or `output: {"type": "json_schema", "schema": {...}}`
- one caller-generated, bounded `request_id` that the server echoes

For example, start an agent turn with:

```json
{
  "request_id": "turn-1",
  "messages": [
    {"role": "system", "content": [{"type": "text", "text": "Use tools to answer questions about tasks."}]},
    {"role": "user", "content": [{"type": "text", "text": "Find the dentist task."}]}
  ],
  "tools": [{
    "name": "search_tasks",
    "description": "Search local tasks",
    "parameters": {
      "type": "object",
      "properties": {"query": {"type": "string"}},
      "required": ["query"],
      "additionalProperties": false
    }
  }],
  "output": {"type": "text"}
}
```

The response has `request_id`, an assistant `message`, `finish_reason`, and `usage` with `input_tokens` and `output_tokens`. Finish reasons are `stop`, `length`, `tool_calls`, and `error`. Truncated or filtered responses can have empty assistant content; it can stay in client history and is omitted when sending to the provider. A tool call has this shape:

```json
{"type": "tool_call", "id": "<opaque-token>", "name": "search_tasks", "arguments": {"query": "dentist"}}
```

Append the **entire returned assistant message unchanged** to the history. Execute the tool on the device, then append its result before making the next request:

```json
{
  "role": "tool",
  "content": [{
    "type": "tool_result",
    "call_id": "<same-opaque-token>",
    "name": "search_tasks",
    "output": "{\"tasks\":[]}"
  }]
}
```

A call ID is an opaque server-issued token that preserves Rig's correlation ID, provider IDs, and replay metadata. Do not replace it with a provider ID or tool name. Tool results must include the tool name because several providers use it when replaying history.

Reasoning blocks have the shape `{"type": "reasoning", "opaque": "<opaque-token>"}`. Echo them unchanged and do not display them. The token preserves complete provider reasoning, including IDs, signatures, and encrypted content. ChatGPT subscription OAuth uses `store: false` and requests encrypted reasoning; the client-held history is needed to continue the conversation. Opaque tokens are not a separate encryption or authorization boundary.

Tool choice is automatic. Requests cannot select a provider, model, credentials, sampling controls, or provider-specific parameters. Images and streaming are not supported. The server does not repair malformed output, switch providers, or run an agent loop. Clients decide how many rounds to run and when to execute or approve tools.

### Tool-capable providers

`features.tools` in `/ai/capabilities` reports whether the ready integration can carry tools. Requests containing tools or tool history return `unsupported_capability` for integrations without tool support: Hyperbolic, Mira, and Perplexity. All other listed integrations are allowed; administrators must choose a model that supports the tools they need.

Two-round native tool calls and result replay were manually tested through the server on October 18, 2026:

| Provider | Model |
| --- | --- |
| ChatGPT subscription OAuth | `gpt-6-astra` |
| OpenAI API | `gpt-4.1-mini` |

Other tool-capable integrations are allowed but untested. These live checks use synthetic tasks, not the iOS app. Offline conversion tests additionally cover encrypted reasoning and signed tool-call replay; CI makes no live provider calls.

Rig providers handle JSON schemas differently. `aven server ai providers` reports these strategies:

- `native-strict` uses the provider's strict structured-output form. Object properties may need nullable types when omission is allowed.
- `native-verbatim` sends the schema without strict rewriting.
- `prompted` adds a provider-neutral instruction containing the caller's schema because the provider lacks native schema support.

Clients must validate returned JSON against their own schema in every case.

## Limits and errors

The server limits each request to 512 KiB of decoded HTTP body, 64 messages, 256 KiB of text (message text, serialized tool arguments, and tool results), 16 tool definitions, and 32 KiB of total tool parameter schemas. Output JSON schemas are limited to 24 KiB. All schemas have a maximum nesting depth of 32. Opaque replay tokens count toward the HTTP body limit, not the text limit. Tool names must be nonempty and definitions must have unique names.

Defaults are 60 calls per minute and two concurrent calls, shared by all clients of the server process. Each agent round consumes one call. Configuration also bounds execution time and output tokens. Rate counters reset on restart.

Aven sends the output-token request to providers that support it. It also checks provider-reported output usage before releasing content. ChatGPT subscription transport does not accept an upstream output-token cap, so Aven can reject an over-limit response but cannot prevent those tokens from being generated and billed. Missing or unverifiable usage fails closed.

Stable error categories distinguish invalid requests, unsupported capabilities, unconfigured service, provider authentication failure, rate limits, timeout, malformed provider responses, canceled requests, and temporary provider failure. Structured provider rejections inside a stream are classified by error code and type even when the transport succeeded. Invalid tool schemas and context-length or input-size violations return `invalid_request` (HTTP 400, `retryable: false`); rate limits return `rate_limit` (HTTP 429, retryable), and overloaded or server errors remain `temporary_provider_failure` (HTTP 503, retryable). Error responses do not include provider error bodies, credentials, or reasoning data. Completion responses carry reasoning only as opaque replay tokens. Logs do not include prompts, schemas, tool arguments, tool results, provider responses, or generated content.

Disconnecting or timing out drops the in-flight provider future. Some providers may continue generation or billing after transport cancellation.

## Privacy

The service sends caller-supplied conversation history, tool definitions, tool results, and any output schema to the configured provider. This context may include transcripts, task titles, notes, project names, and labels. A client that transcribes audio locally sends the transcript, not the microphone audio. The server cannot restrict which information a client includes in its messages.

Aven logs bounded operational metadata such as provider category, outcome, duration, and reported token counts. It suppresses Rig's content-bearing diagnostic logs even when verbose Aven logging is enabled.
