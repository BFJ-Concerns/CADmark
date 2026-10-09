# CADmark

AI-directed CAD modelling with spatial comments. The user selects geometry
in a 3D viewport and writes comments anchored to the selection; the system
translates selections to code context for the AI, which modifies a build123d
Python script.

## Architecture

Rust workspace with six crates:

| Crate | Responsibility |
|-------|---------------|
| `cadmark-core` | Domain types: provenance ledger, geometry context, spatial comments, messages, microversions |
| `cadmark-kernel` | PyO3 bridge to build123d — script execution, OCP instrumentation, tessellation extraction |
| `cadmark-renderer` | wgpu pipeline — shaded mesh, wireframe overlay, GPU colour-ID picking, selection glow |
| `cadmark-ui` | egui panels — chat pane, comment overlay, undo/redo toolbar |
| `cadmark-bridge` | AI backend trait, strict configuration, and shared OpenAI-compatible client |
| `cadmark-app` | Binary entry point, state management, orchestrator, git operations |

## Build & Run

`justfile` wraps every routine command; `just` on its own lists the recipes.

```bash
just bootstrap       # One-off: create .venv with build123d + OCP
just check           # Type-check
just test            # Run all tests (kernel tests need the .venv)
just verify          # Formatting, lints, and tests
just run [dir]       # Run; with a folder named, open it directly
just install         # Build release and install as a desktop application
```

Two CI workflows run `just verify` with `CADMARK_TEST_RUNNER=nextest`, which
swaps `cargo test` for cargo-nextest's retrying `ci` profile plus the
doctests. `.forgejo/workflows/ci.yml` is the development gate: every pull
request and branch push on the Forgejo origin. `.github/workflows/ci.yml` is
the public gate: pushes to `main` and pull requests on GitHub, on a stock
Ubuntu runner. Checks belong in the justfile, not in either workflow, so the
two gates cannot drift from each other or from a dev box.

Requires Python 3.12 (cadquery-ocp). The `.cargo/config.toml` points PyO3
at the repository `.venv`'s interpreter, a symlink the build scripts follow
to the real Python 3.12 installation. `cadmark-app` embeds an rpath
to that runtime's `lib/` directory, and `cadmark-kernel` sets `PYTHONHOME`
from the configured interpreter before Python initialises. The gitignored
`.venv/` in the project root holds the runtime packages (`build123d`, OCP)
pinned in `requirements.txt`; `scripts/bootstrap-python-runtime` creates it.
Without it the kernel tests fail with `No module named 'OCP'` and the app
cannot execute scripts.

## Key Decisions

- **Configured provider boundary**: Both AI consumers share one
  OpenAI Responses-compatible client behind `TurnModel`. The user settings
  store (`~/.config/cadmark/settings.json`, `SettingsStore::default_location`)
  selects the endpoint and model; the credential is read from
  `CADMARK_AI_API_KEY` when set, otherwise from the owner-only credential
  file beside the settings. No project folder carries AI configuration.
- **Provenance via OCP instrumentation**: Wraps OCP builder
  classes to capture which source lines generated which geometry.
- **Geometry context as experimental layer**: Three-layer
  architecture — provenance (foundation), identification (modular/experimental),
  stable output format. `IdentificationStrategy` trait for pluggable strategies.

## Testing

- Unit tests in each crate (`#[cfg(test)]` modules)
- Git operations tested with `tempfile` temporary repos
- Kernel tests require Python 3.12 with build123d installed
- Renderer pipeline tests require wgpu (GPU or software adapter)

## Conventions

- Edition 2024 Rust
- `thiserror` for library error types, `anyhow` for application errors
- PyO3 0.24 — `py.run()` takes `&CStr`, use `c"..."` literals for static Python
- Commit metadata: `---cadmark---` separator, `trigger:` and `type:` fields
