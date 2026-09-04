# CADmark

AI-directed CAD modelling with spatial comments. Select geometry in a 3D viewport, write what you want changed, and an AI edits the build123d Python script that defines the model.

## Install

CADmark needs Rust, Python 3.12, and a GPU (or software adapter).

Set up the Python runtime once:

```sh
scripts/bootstrap-python-runtime
```

This creates a `.venv` at the repository root with the pinned versions of build123d and cadquery-ocp that the embedded kernel requires.

Build and run:

```sh
cargo build
cargo run            # opens the start view
cargo run -- mypart  # opens a project folder directly
```

Run the tests (kernel tests need the `.venv`):

```sh
cargo test
```

## Connect an AI provider

Open Settings (`Ctrl+,`) and enter the base URL, model name, and API key for any OpenAI Responses-compatible endpoint — OpenAI, a gateway in front of Claude or Gemini, OpenRouter, or a local model server. See the [configuration reference](docs/reference/configuration.md) for the full settings surface, credential handling, and error reporting.

Without a provider configured, the model still loads and rebuilds — only the AI chat is unavailable.

## Use

A project is a folder holding one or more build123d scripts, each defining a part. Open or create a project from the start view, then describe what you want in the chat. The AI writes a script, executes it, and the result appears in the viewport.

Click a face, edge, or vertex to select it and write a spatial comment anchored to that geometry — the AI sees the source line that produced the selected element. Pending comments are sent together with chat text as one turn.

The parameters panel lists every named number in the script; drag a value to change it directly without an AI turn. Every accepted edit is a design step — undo with `Ctrl+Z`, name a version with `Ctrl+S`.

The section plane and ghost mode let you inspect internal geometry. Export to STEP, STL, or 3MF from the toolbar.

## Documentation

- [Getting started](docs/guides/getting-started.md) — install, connect a provider, and make your first edit
- [Configuration reference](docs/reference/configuration.md) — settings, credentials, limits, and provider errors
- [Interface reference](docs/reference/interface.md) — viewport, toolbar, chat, panels, measurement, and shortcuts
