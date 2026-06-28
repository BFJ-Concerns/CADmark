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
| `cadmark-bridge` | AI backend trait + Claude Code subprocess implementation |
| `cadmark-app` | Binary entry point, state management, orchestrator, git operations |

## Build & Run

```bash
cargo check          # Type-check
cargo test           # Run all tests
cargo run -- [dir]   # Run with a project directory (defaults to cwd)
```

Requires Python 3.12 (cadquery-ocp). The `.cargo/config.toml` points PyO3
at the uv-managed Python 3.12 installation. `cadmark-app` embeds an rpath
to that runtime's `lib/` directory, and `cadmark-kernel` sets `PYTHONHOME`
from the configured interpreter before Python initialises. The `.venv/` in
the project root is for runtime packages such as `build123d`.

## Key Decisions

- **Claude Code as AI backend** (ADR-0001): Delegates LLM interaction to
  `claude --print` subprocess. Behind an `AiBackend` trait for replaceability.
- **Provenance via OCP instrumentation** (ADR-0002): Wraps OCP builder
  classes to capture which source lines generated which geometry.
- **Geometry context as experimental layer** (ADR-0003): Three-layer
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
