---
description: Settings file, credential handling, execution limits, and context-window configuration
---

# Configuration reference

## Settings location

`$XDG_CONFIG_HOME/cadmark/settings.json`, falling back to `~/.config/cadmark/settings.json`. Created on first save. A missing file is treated as defaults; a malformed file reports the field and location without echoing the file's contents.

Source: `crates/cadmark-app/src/user_settings.rs:86–99`, `crates/cadmark-app/src/user_settings.rs:109–131`.

## Settings fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `ai.base_url` | string | (none) | OpenAI Responses-compatible base URL |
| `ai.model` | string | (none) | Model name |
| `ai.accepts_images` | bool | `false` | Whether the model reads images; sends message attachments and enables the render and reference-library tools |
| `ai.allow_insecure_http` | bool | `false` | Permit a plain-HTTP endpoint (local model servers) |
| `ai.reasoning_effort` | string | (none) | Sent as the request's `reasoning.effort` on every call when set (`low`, `medium`, `high`, or whatever the endpoint accepts); unset, the field is not sent and the provider's default applies |
| `limits.wall_clock` | duration | 120 s | Script execution wall-clock ceiling |
| `limits.memory_bytes` | integer | 4294967296 (4 GB) | Script execution resident-memory ceiling in bytes |
| `context_window_tokens` | integer | 128000 | Fallback context window, used only when the endpoint does not advertise one for the model (see below); CADmark condenses conversation before approaching the limit in force |
| `recent_projects` | array of paths | `[]` | Recently opened project folders, most recent first; capped at 8 |

Source: `crates/cadmark-app/src/user_settings.rs:32–43`, `crates/cadmark-bridge/src/config.rs` (`AiConfiguration`), `crates/cadmark-core/src/limits.rs:9–17`, `crates/cadmark-app/src/user_settings.rs:29`.

## Settings dialog

The dialog opens from the toolbar gear button or from the "AI off" badge. It is a floating window — the viewport and chat remain usable while it is open. Keyboard shortcut: `Ctrl+,`.

Sections:

- **AI provider** — base URL, model, reasoning effort (text; blank sends nothing), credential (masked, never echoed), accepts-images checkbox, allow-insecure-HTTP checkbox. The hint text notes "Any OpenAI Responses-compatible endpoint: OpenAI, a gateway in front of Claude or Gemini, OpenRouter, or a local model server."
- **Script limits** — wall clock (seconds, min 1, max 3600) and memory (MB, min 64, max 65536).
- **Conversation context** — context window in tokens (min 1024, max 10000000).

Validation failures keep the dialog open with an inline error message.

Source: `crates/cadmark-ui/src/settings_dialog.rs`.

## Credential

The credential is stored separately from settings in `$XDG_CONFIG_HOME/cadmark/credential`, a plain-text file with owner-only permissions (mode `0600` on Unix). The settings file never carries it.

The environment variable `CADMARK_AI_API_KEY`, when set and non-empty, overrides the stored credential. Resolution order: environment variable → stored file → none.

An empty credential value in the dialog leaves the stored file as it is; the dialog offers no way to remove a stored credential, so deleting the file is how one is removed. The dialog shows which source is active ("set by the environment variable", "stored", or prompts for an API key).

Source: `crates/cadmark-app/src/user_settings.rs:20–21`, `crates/cadmark-app/src/user_settings.rs:139–193`.

## Context window detection

On each project open the orchestrator probes the endpoint for the configured model's context window (`OpenAiCompatibleClient::context_window`): `GET {base_url}/models/{model}` first, then `GET {base_url}/models` searched by `id`, then the same list requested with an `anthropic-version: 2023-06-01` header (Anthropic's list format, the only one in which gateways fronting Claude, such as CLIProxyAPI, report `max_input_tokens`). Recognised fields: `context_window`, `context_length`, `max_context_length`, `context_window_tokens`, `max_input_tokens`, `n_ctx`, `top_provider.context_length` (OpenRouter), `meta.n_ctx_train` (llama.cpp), `metadata.context_length`, and any `model_info` key ending in `.context_length` (Ollama). Values below 1024 are ignored. A found value arrives as `OrchestratorResult::ContextWindowDetected` and is held on the project (`Project::detected_context_window`), taking precedence over `context_window_tokens` for every turn and for the chat's occupancy figure; a failed or empty probe leaves the setting in force and is never an error.

Source: `crates/cadmark-bridge/src/openai_compatible.rs` (`context_window`, `advertised_context_window`), `crates/cadmark-app/src/orchestrator.rs`, `crates/cadmark-app/src/app.rs` (`context_window_tokens`).

## Provider refusals

When the provider refuses a request, the cause is identified and reported:

| Cause | Message |
|-------|---------|
| Usage limit / rate limit / quota | "the provider is at its usage limit or cooling down" |
| Overloaded (HTTP 503/529, `overloaded_error`, "server is busy", "no slots available") | "the provider is overloaded or busy; try again shortly" |
| Authentication failure | "the provider rejected the credential" |
| Unknown model | "the provider does not serve the configured model" |

When no provider is configured: "no AI provider is configured; open Settings to add one". The model still loads and rebuilds without AI.

Source: `crates/cadmark-bridge/src/backend.rs:19–36`, `crates/cadmark-app/src/app.rs` (`ai_services`).
