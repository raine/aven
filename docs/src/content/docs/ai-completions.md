---
title: AI completion server
description: Configure Aven's generic, self-hosted AI completion service.
---

The Aven server can expose a generic text completion service for paired clients. The service forwards caller-supplied text and a JSON schema to an administrator-selected model. It does not read tasks, projects, labels, attachments, or the sync operation log, and it does not create or modify Aven data.

AI is optional. An unconfigured AI service does not prevent the sync server from starting or serving sync requests.

## Configure the service

Add an `ai` section to the server's `config.yaml`:

```yaml
sync:
  auth_token: "shared-pairing-secret"

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

The server chooses the provider, model, endpoint, timeout, and limits. Completion requests cannot override them. `ai.endpoint` is available for llamafile, Ollama, and OpenAI-compatible test or gateway deployments; other providers use Rig's provider-specific environment and endpoint rules.

## Providers and credentials

Run this command to see providers available in the pinned Rig release, their credential environment variable, and their schema strategy:

```sh
aven ai providers
```

Aven supports the Rig 0.42.0 text-completion modules for Anthropic, Azure OpenAI, ChatGPT subscription OAuth, Cohere, GitHub Copilot, DeepSeek, Doubleword, Gemini, Groq, Hugging Face, Hyperbolic, llamafile, MiniMax, Mira, Mistral, Moonshot, Ollama, OpenAI, OpenRouter, Perplexity, Together, Venice, xAI, Xiaomi MiMo, and Z.AI. Voyage AI is excluded because its Rig module provides embeddings and reranking but no text completion model.

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

The long-running server disables interactive device flow. Missing OAuth material produces `authentication_required` without blocking for terminal input. Expired or invalid credentials fail the provider request without starting an interactive flow.

Inspect local readiness without contacting or billing a provider:

```sh
aven ai status
aven ai status --json
```

Make one synthetic, billable provider request after setup:

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

Stable error categories distinguish invalid requests, unsupported capabilities, unconfigured service, provider authentication failure, rate limits, timeout, malformed provider responses, canceled requests, and temporary provider failure. Responses and logs do not include provider error bodies, credentials, reasoning data, prompts, schemas, or generated content.

Disconnecting or timing out drops the in-flight provider future. Some providers may continue generation or billing after transport cancellation.

## Privacy

The service sends caller-supplied message text and the requested JSON schema to the configured provider. A future iPhone drafting client may include a transcript, project names, label names, and unsaved form context in that text. Microphone audio remains on the iPhone, but the generic server cannot inspect or enforce what a client includes in its messages.

Aven logs bounded operational metadata such as provider category, outcome, duration, and reported token counts. It suppresses Rig's content-bearing diagnostic logs even when verbose Aven logging is enabled.

## Packaging evidence

The host-only Rig dependency remains in the combined `aven` executable and does not enter `aven-core`. Clean `aarch64-apple-darwin` release builds with rustc 1.96.0 measured:

| Stage | Unstripped | `strip -x` | Release `.tar.gz` | Clean build |
| --- | ---: | ---: | ---: | ---: |
| Baseline | 34,291,504 B | 30,175,200 B | 13,821,699 B | 99.72 s |
| Rig plus Ollama path | 34,709,712 B | 30,458,080 B | 13,989,799 B | 94.12 s |
| Complete registry | 40,183,712 B | 35,017,880 B | 15,844,051 B | 142.70 s |

The complete artifact adds about 2.0 MB, or 14.6 percent, to the compressed baseline. Clean-build timing varied from 94 to 143 seconds with machine load and was not a packaging constraint. The registry adds 27 normal dependency-list entries, introduces no second Reqwest version, and does not enable Reqwest's system-proxy or HTTP/2 features. This measured cost is modest enough to retain the existing combined executable.

## Troubleshooting

- `unconfigured`: set both `ai.provider` and `ai.model`.
- `authentication_required`: provide the listed API-key environment variable or run `aven ai login` for the configured OAuth provider.
- `temporarily_unavailable`: verify provider-specific environment, endpoint, credential-directory access, and model availability.
- HTTP 401 on AI routes: configure `sync.auth_token` and send the matching bearer token.
- `malformed_provider_response`: verify that the selected model returns text, reports token usage, and can follow the requested schema.
- `rate_limit`: retry later. This category can come from Aven's local admission controls or the provider.

Manual task capture and sync continue to work when AI is unavailable.
