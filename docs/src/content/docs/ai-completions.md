---
title: AI completion server
description: Configure Aven's generic, self-hosted AI completion service.
---

The completion server lets paired clients use an administrator-configured AI provider without storing provider credentials on each client. A client sends text and a JSON schema, and the server returns model-generated text with provider-neutral usage and finish information.

Aven for iOS uses this service to turn typed or locally transcribed instructions into a structured task draft. The iOS app is not publicly released yet. It supplies the drafting instructions and validates the result. The server remains generic: it does not read tasks, projects, labels, attachments, or the sync operation log, and it does not create or modify Aven data.

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
  requests_per_minute: 10
```

`sync.auth_token` is required for AI, including on loopback. It authorizes only the AI routes; end-to-end encrypted sync uses separate device credentials. Every client given this shared AI token can use configured model capacity, and Aven cannot grant AI permission to only some clients that share it.

The server chooses the provider, model, endpoint, timeout, and limits. Completion requests cannot override them. `ai.endpoint` is available for llamafile, Ollama, and custom OpenAI-compatible endpoints. Other providers use their standard endpoints.

## Providers and credentials

List the available providers, credential environment variables, and JSON-schema strategies:

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

Completion contract 1 accepts only:

- system and user text messages
- one caller-generated, bounded request ID that the server echoes
- one JSON schema for the text response

It does not accept provider or model selection, credentials, sampling controls, images, tools, tool results, streaming, or provider-specific parameters. The server performs exactly one provider request. It does not repair malformed output, switch providers, or run an agent loop.

Rig providers handle JSON schemas differently. `aven ai providers` reports these strategies:

- `native-strict` uses the provider's strict structured-output form. Object properties may need nullable types when omission is allowed.
- `native-verbatim` sends the schema without strict rewriting.
- `prompted` adds a provider-neutral instruction containing the caller's schema because the provider lacks native schema support.

Clients must validate returned JSON against their own schema in every case.

## Limits and errors

The server limits requests to 65,536 decoded bytes, eight messages, eight text blocks per message, 48 KiB of message text, 24 KiB of schema JSON, and a schema nesting depth of 32. Configuration bounds execution time, concurrent calls, calls per minute, and output tokens. Limits are process-local and rate counters reset on restart.

Aven sends the output-token request to providers that support it. It also checks provider-reported output usage before releasing content. ChatGPT subscription transport does not accept an upstream output-token cap, so Aven can reject an over-limit response but cannot prevent those tokens from being generated and billed. Missing or unverifiable usage fails closed.

Stable error categories distinguish invalid requests, unsupported capabilities, unconfigured service, provider authentication failure, rate limits, timeout, malformed provider responses, canceled requests, and temporary provider failure. Responses do not include provider error bodies, credentials, or reasoning data. Logs do not include prompts, schemas, provider responses, or generated content.

Disconnecting or timing out drops the in-flight provider future. Some providers may continue generation or billing after transport cancellation.

## Privacy

The service sends caller-supplied message text and the requested JSON schema to the configured provider. Clients may include transcripts, project names, label names, and unsaved form context in that text. A client that transcribes audio locally sends the transcript, not the microphone audio. The server cannot restrict which information a client includes in its messages.

Aven logs bounded operational metadata such as provider category, outcome, duration, and reported token counts. It suppresses Rig's content-bearing diagnostic logs even when verbose Aven logging is enabled.
