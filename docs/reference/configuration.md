---
description: How to configure CADmark's AI provider, script limits, and credentials
---

# Configuration

CADmark stores its settings in `settings.json` inside your configuration directory — `$XDG_CONFIG_HOME/cadmark/` or, on most systems, `~/.config/cadmark/`. The file is created the first time you save settings; until then, defaults apply.

You rarely need to edit the file directly. The settings dialog (gear button in the toolbar, or `Ctrl+,`) covers everything.

## AI provider

CADmark talks to any OpenAI Responses-compatible endpoint — OpenAI itself, a gateway in front of Claude or Gemini, OpenRouter, or a local model server. Set:

- **Base URL** — the endpoint, for example `https://api.openai.com/v1`.
- **Model** — the model name your endpoint serves.
- **Accepts images** — tick this if the model can read images. It enables the AI's render tool (the model can look at the model it built) and sends PNG and JPEG reference images from the project's `references/` folder.
- **Allow insecure HTTP** — only needed for a local server on `http://`.

Without a provider configured, the toolbar shows "AI off". The model still loads and rebuilds; you can edit parameters and export — the AI chat is the only thing unavailable.

## Credential

The API key is stored separately from settings, in a file called `credential` in the same directory, with owner-only permissions (`0600`). The settings file never carries it.

To use a key without storing it, set the `CADMARK_AI_API_KEY` environment variable — it takes precedence over the stored file. The settings dialog tells you which source is active.

## Script limits

Every time the AI writes and runs a build123d script, two ceilings apply:

| Limit | Default | Range |
|-------|---------|-------|
| Wall clock | 120 seconds | 1–3600 s |
| Resident memory | 4096 MB | 64–65536 MB |

A script that hits either ceiling is stopped, and the AI is told which one it crossed so it can try a lighter approach.

## Context window

Set this to match your model's context window (default: 128 000 tokens). CADmark condenses the conversation before it reaches this limit, keeping decisions and outstanding requests.

## What happens when something goes wrong

If the provider rejects a request, the chat tells you why:

- **Usage limit** — "the provider is at its usage limit or cooling down" (HTTP 429 or quota messages)
- **Authentication** — "the provider rejected the credential"
- **Unknown model** — "the provider does not serve the configured model"

A malformed settings file reports the file path, the affected field, and the line and column to look at — without echoing the file's contents.
