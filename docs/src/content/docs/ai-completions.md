---
title: AI completion server
description: Configure Aven's generic, self-hosted AI completion service.
---

The completion server lets paired clients use an administrator-configured AI provider without storing provider credentials on each client. It accepts conversation history and tool definitions and returns text, native tool calls, and opaque reasoning. JSON-schema output is also available.

The iOS agent runs its loop on the device: it sends a completion request, executes the returned tools against its local tasks, then sends the results with the conversation history for the next completion. End-to-end encrypted sync keeps task data readable only on the device; the device chooses what context to send to AI. The server does not read the sync database, execute tools, or create or modify Aven data. It performs one provider call per HTTP request and keeps no conversation or turn state.

AI is optional. An unconfigured AI service does not prevent the sync server from starting or serving sync requests.

## Configure the service

Add an `ai` section to the server's `config.yaml`:

```yaml
sync:
  auth_token: "shared-ai-secret"

ai:
  provider: ollama
  model: llama3.2
  endpoint: "http://127.0.0.1:11434"
  timeout_seconds: 45
  max_output_tokens: 2048
  max_concurrent_requests: 2
  requests_per_minute: 60
```

`sync.auth_token` is required for AI, including on loopback. It authorizes only the AI routes; end-to-end encrypted sync uses separate device credentials. Every client given this shared AI token can use configured model capacity, and Aven cannot grant AI permission to only some clients that share it.

The server chooses the provider, model, endpoint, timeout, and limits. Completion requests cannot override them. `ai.endpoint` is available for llamafile, Ollama, and custom OpenAI-compatible endpoints. Other providers use their standard endpoints.

## Providers and credentials

List the available providers, credential environment variables, tool support, and JSON-schema strategies:

```sh
aven ai providers
```

The provider registry includes Anthropic, Azure OpenAI, ChatGPT subscription OAuth, Cohere, GitHub Copilot, DeepSeek, Doubleword, Gemini, Groq, Hugging Face, Hyperbolic, llamafile, MiniMax, Mira, Mistral, Moonshot, Ollama, OpenAI, OpenRouter, Perplexity, Together, Venice, xAI, Xiaomi MiMo, and Z.AI. Voyage AI is excluded because it does not provide text completions.

API-key providers read the environment variable shown by `aven ai providers` from the server process. Azure also uses `AZURE_ENDPOINT` and `AZURE_API_VERSION`. Keep these values in the service manager's protected secret environment, not in `config.yaml`.

OAuth providers store material below:

```text
$AVEN_CONFIG_DIR/private/ai/
```

When `AVEN_CONFIG_DIR` is unset, the base directory is `~/.config/aven`. Aven sets the private AI directory to owner-only permissions on Unix. It does not include this directory in sync, backups, or exports.

Start an interactive OAuth flow only from an administrator terminal:

```sh
aven ai login chatgpt
aven ai login copilot
```

Login stores credentials but does not select the server provider or model. Set `ai.provider` to `chatgpt` or `copilot`, and set `ai.model` to a model available to the authenticated account.

The long-running server does not start an interactive login. Missing OAuth material produces `authentication_required`. Expired or invalid credentials fail the provider request.

### Log out locally

Remove locally stored OAuth credentials with:

```sh
config_dir="${AVEN_CONFIG_DIR:-$HOME/.config/aven}"
rm -f "$config_dir/private/ai/chatgpt/auth.json"
rm -f "$config_dir/private/ai/copilot/access-token" \
  "$config_dir/private/ai/copilot/api-key.json"
```

This removes Aven's local credentials. It does not revoke access at the provider. Use the provider's account settings when you also need to revoke the authorization.

Inspect local configuration and credential presence without contacting or billing a provider:

```sh
aven ai status
aven ai status --json
```

A `ready` status does not verify that the configured model is available to the account. Make one synthetic, billable request to verify the complete setup:

```sh
aven ai check
```

## HTTP contract

Both routes require `Authorization: Bearer <sync.auth_token>`:

```text
GET  /ai/capabilities
POST /ai/completions
```

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

Rig providers handle JSON schemas differently. `aven ai providers` reports these strategies:

- `native-strict` uses the provider's strict structured-output form. Object properties may need nullable types when omission is allowed.
- `native-verbatim` sends the schema without strict rewriting.
- `prompted` adds a provider-neutral instruction containing the caller's schema because the provider lacks native schema support.

Clients must validate returned JSON against their own schema in every case.

## Limits and errors

The server limits each request to 512 KiB of decoded HTTP body, 64 messages, 256 KiB of text (message text, serialized tool arguments, and tool results), 16 tool definitions, and 32 KiB of total tool parameter schemas. Output JSON schemas are limited to 24 KiB. All schemas have a maximum nesting depth of 32. Opaque replay tokens count toward the HTTP body limit, not the text limit. Tool names must be nonempty and definitions must have unique names.

Defaults are 60 calls per minute and two concurrent calls, shared by all clients of the server process. Each agent round consumes one call. Configuration also bounds execution time and output tokens. Rate counters reset on restart.

Aven sends the output-token request to providers that support it. It also checks provider-reported output usage before releasing content. ChatGPT subscription transport does not accept an upstream output-token cap, so Aven can reject an over-limit response but cannot prevent those tokens from being generated and billed. Missing or unverifiable usage fails closed.

Stable error categories distinguish invalid requests, unsupported capabilities, unconfigured service, provider authentication failure, rate limits, timeout, malformed provider responses, canceled requests, and temporary provider failure. Error responses do not include provider error bodies, credentials, or reasoning data. Completion responses carry reasoning only as opaque replay tokens. Logs do not include prompts, schemas, tool arguments, tool results, provider responses, or generated content.

Disconnecting or timing out drops the in-flight provider future. Some providers may continue generation or billing after transport cancellation.

## Privacy

The service sends caller-supplied conversation history, tool definitions, tool results, and any output schema to the configured provider. This context may include transcripts, task titles, notes, project names, and labels. A client that transcribes audio locally sends the transcript, not the microphone audio. The server cannot restrict which information a client includes in its messages.

Aven logs bounded operational metadata such as provider category, outcome, duration, and reported token counts. It suppresses Rig's content-bearing diagnostic logs even when verbose Aven logging is enabled.
