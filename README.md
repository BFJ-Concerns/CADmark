# CADmark

AI-directed CAD modelling with spatial comments. Select geometry in a 3D viewport, write what you want changed, and an AI edits the build123d Python script that defines the model.

## Install

CADmark needs Rust, Python 3.12, and a GPU (or software adapter). The
[`just`](https://github.com/casey/just) recipes below are the shortest route;
each one wraps ordinary `cargo` and `scripts/` commands that work on their own.

Set up the Python runtime once:

```sh
just bootstrap
```

This creates a `.venv` at the repository root with the pinned versions of build123d and cadquery-ocp that the embedded kernel requires.

Build and run from the checkout:

```sh
just run          # opens the start view
just run mypart   # opens a project folder directly
```

Run the tests (kernel tests need the `.venv`):

```sh
just test         # or `just verify` for formatting and lints as well
```

Install CADmark as a desktop application for the current user:

```sh
just install
```

This builds the workspace in release mode, copies both binaries and a Python
runtime of their own into `~/.local/lib/cadmark`, adds a `cadmark` launcher to
`~/.local/bin`, and registers a menu entry and icon — so the installed copy is
independent of the checkout. Set `CADMARK_PREFIX` to install elsewhere.
`just uninstall` removes all of it.

`just` on its own lists every recipe.

## Connect an AI provider

Open Settings (`Ctrl+,`) and enter the base URL, model name, and API key for any OpenAI Responses-compatible endpoint — OpenAI, a gateway in front of Claude or Gemini, OpenRouter, or a local model server. See the [configuration reference](docs/reference/configuration.md) for the full settings surface, credential handling, and error reporting.

Without a provider configured, the model still loads and rebuilds — only the AI chat is unavailable.

## Use

A project is a folder holding one or more build123d scripts, each defining a part. Open or create a project from the start view, then describe what you want in the chat. The AI writes a script, executes it, and the result appears in the viewport. After each build, the status bar reports each part's validity — "Part 1 is closed and valid." or "Part 1 is NOT a closed valid solid; it will not print."

Click a face, edge, or vertex to select it and write a spatial comment anchored to that geometry — the AI sees the source line that produced the selected element. The Select menu in the toolbar turns each kind of click target on or off, so a disabled kind's click falls through to what is behind it. Pending comments are sent together with chat text as one turn.

The parameters panel lists every named number in the script; drag a value to change it directly without an AI turn. Every accepted edit is a design step — undo with `Ctrl+Z`, name a version with `Ctrl+S`.

Use **Attach images…** above chat to add [project reference photos and drawings](docs/reference/interface.md#reference-images) for the AI. They remain available across conversations. The section plane and ghost mode let you inspect internal geometry. Export to STEP, STL, or 3MF from the toolbar.

## Documentation

Use `/3d-printing` or `$3d-printing` at the start of a message for a printing
review or a design request guided by filament-printing constraints. The chat
pane’s Skills menu lists the built-in skill.

- [Getting started](docs/guides/getting-started.md) — install, connect a provider, and make your first edit
- [Configuration reference](docs/reference/configuration.md) — settings, credentials, limits, and provider errors
- [Interface reference](docs/reference/interface.md) — viewport, toolbar, chat, panels, measurement, and shortcuts
- [3D printing](docs/guides/3d-printing.md) — built-in advice, orientation, and fit checks
- [Common modelling tasks](docs/guides/modelling.md) — threaded bolts, helix sweeps, holes, enclosures, and fillets
