---
description: Build prerequisites, Python runtime setup, and launching CADmark
unpaired: human readers get a getting-started guide instead
---

# Installation and build

## Prerequisites

- Rust (edition 2024 workspace)
- Python 3.12 — cadquery-ocp publishes no wheels for newer versions
- A GPU or software adapter supported by wgpu

## Python runtime

The embedded kernel imports build123d from a `.venv` at the repository root. Create it once:

```
scripts/bootstrap-python-runtime
```

The script finds Python 3.12 in this order: `--python PATH` argument, the `PYO3_PYTHON` environment variable, `uv python find 3.12`, then `python3.12` on `PATH`. It installs the pinned versions from `requirements.txt` (`build123d==0.11.1`, `cadquery-ocp-novtk==7.9.3.1.1`) and verifies that `build123d` and `OCP` import.

Re-running is safe: an existing `.venv` is reused and brought up to the pinned versions.

Source: `scripts/bootstrap-python-runtime`, `requirements.txt`.

## PyO3 configuration

`.cargo/config.toml` sets `PYO3_PYTHON` to `.venv/bin/python` (relative to the repo root). A `PYO3_PYTHON` environment variable overrides it.

`cadmark-app` embeds an rpath to the Python 3.12 runtime's `lib/` directory; `cadmark-kernel` sets `PYTHONHOME` from the configured interpreter before Python initialises.

Source: `.cargo/config.toml`, `crates/cadmark-app/build.rs`, `crates/cadmark-kernel/src/python_runtime.rs`.

## Build and run

```
cargo check                         # type-check
cargo test                          # run all tests (kernel tests need the .venv)
cargo run --bin cadmark             # run — opens the start view
cargo run --bin cadmark -- [dir]    # run with a project directory
```

The workspace has two binaries (`cadmark` in cadmark-app, `cadmark-kernel-worker` in cadmark-kernel) and no `default-run`, so `--bin cadmark` is required. With no directory argument, CADmark opens the start view. A directory argument opens that project folder directly.

Source: `crates/cadmark-app/Cargo.toml:6–8`, `crates/cadmark-kernel/Cargo.toml:6–7`, `crates/cadmark-app/src/main.rs:34–64`, `crates/cadmark-app/src/launch.rs`.
