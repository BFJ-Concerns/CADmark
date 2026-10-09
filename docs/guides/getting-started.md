---
description: Install CADmark, connect an AI provider, and make your first edit
unpaired: tutorial for new users — the agent set covers the same facts as terse reference
---

# Getting started

This guide walks through installing CADmark, connecting it to an AI provider, and making a first edit to a 3D model.

## What you need

- **Linux** — with Landlock enabled, a Wayland or X11 session, and xdg-desktop-portal with a backend that provides the file chooser. The [README](../../README.md#requirements) lists the platform requirements and the system packages to install for Debian, Ubuntu, Fedora, and Arch.
- **Rust** — a recent stable toolchain (the workspace uses edition 2024).
- **Python 3.12** — exactly: the kernel's provenance instrumentation is validated against Python 3.12 with the pinned build123d and OCP releases, and refuses any other runtime.
- **A GPU** — or a software adapter that wgpu can use.
- **An OpenAI Responses-compatible API endpoint** — OpenAI, a gateway in front of Claude or Gemini, OpenRouter, or a local model server.

[`just`](https://github.com/casey/just) is optional but shortens every command below; `just` on its own lists the available recipes.

## Set up the Python runtime

CADmark embeds a Python runtime to run build123d scripts. The bootstrap script creates a virtual environment at the repository root with the exact library versions the kernel expects:

```sh
scripts/bootstrap-python-runtime   # or: just bootstrap
```

It finds a Python 3.12 interpreter automatically (or accepts `--python /path/to/python3.12`), installs the pinned dependencies, and confirms that `build123d` and `OCP` import.

## Build

```sh
cargo build
```

This builds both binaries in the debug profile: the `cadmark` application and the `cadmark-kernel-worker` it runs scripts in. The application starts the worker from its own directory, so build the whole workspace in the same profile before launching with `cargo run`. (`just build` builds the release profile instead, for `just install`.)

To run the test suite (some kernel tests need the `.venv`):

```sh
cargo test    # or: just test
```

## Launch

```sh
cargo run --bin cadmark   # or: just run, which builds both binaries first
```

CADmark opens the start view. From here you can create a new project in an empty folder or open an existing one. To skip the start view and open a folder directly:

```sh
cargo run --bin cadmark -- /path/to/project   # or: just run /path/to/project
```

## Install it properly

Running from the checkout is fine for trying CADmark out. To have it in the
applications menu like any other program:

```sh
just install
```

CADmark is installed for your user under `~/.local` — binaries and their own
Python runtime in `~/.local/lib/cadmark`, a `cadmark` command in
`~/.local/bin`, and a menu entry with an icon. Nothing needs root, and the
installed copy keeps working if you move or rebuild the checkout. Set
`CADMARK_PREFIX` to install somewhere else, and run `just uninstall` to remove
it again.

Re-run `just install` whenever you want the installed copy brought up to the
current source.

The installed copy keeps a log at `~/.local/state/cadmark/cadmark.log` (or
under `XDG_STATE_HOME` if you set it), with a line for each AI request as it
starts and ends. When it has grown past five megabytes, the launcher starts it afresh at the next launch.

## Connect an AI provider

Open Settings (the gear button, or `Ctrl+,`) and fill in:

- **Base URL** — your endpoint, for example `https://api.openai.com/v1`.
- **Model** — the model your endpoint serves.
- **Credential** — the API key. It is stored in a separate file with restricted permissions, never inside the settings file.

Tick "The model reads images" if the model supports vision — this lets the AI render and inspect the model it builds, read the images you attach to messages, and keep a reference library for the project.

Save, and the toolbar shows the model name where "AI off" was.

> [!TIP]
> You can also set the `CADMARK_AI_API_KEY` environment variable instead of storing a key. It takes precedence over the stored credential.

## Your first project

1. Click "New project" and pick an empty folder.
2. Type something in the chat — "a box with rounded edges, 80 mm wide" — and press Enter.
3. The AI writes a build123d script, executes it, and the model appears in the viewport.
4. Click a face, type a comment in the overlay ("make this face thinner"), and press Enter. The comment becomes a pending card in the chat pane.
5. Click **Send 1 comment** (or press Enter in the chat input). The AI sees which face you selected and which source line produced it, and edits the script accordingly.

Every accepted edit is recorded as a design step. Undo with `Ctrl+Z`, redo with `Ctrl+Shift+Z`, or name a version with `Ctrl+S` to mark a checkpoint you can return to.

## Next steps

- [Configuration reference](../reference/configuration.md) — the full settings surface, credential handling, and how provider errors are reported.
- [Interface reference](../reference/interface.md) — every control, panel, shortcut, and measurement.
