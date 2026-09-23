---
description: Build prerequisites, Python runtime setup, task runner, launching, and desktop installation
unpaired: human readers get a getting-started guide instead
---

# Installation and build

## Task runner

`justfile` at the repository root wraps every command below —
`just bootstrap`, `just check`, `just test`, `just lint`, `just run [dir]`,
`just verify` (formatting, lints, tests), `just build`, `just install`,
`just uninstall`. `just` with no argument lists them. The underlying `cargo`
and `scripts/` commands work unchanged without it.

Source: `justfile`.

## Prerequisites

- Rust (edition 2024 workspace)
- Python 3.12 — cadquery-ocp publishes no wheels for newer versions
- A GPU or software adapter supported by wgpu

## Python runtime

The embedded kernel imports build123d from a `.venv` at the repository root. Create it once:

```
scripts/bootstrap-python-runtime
```

The script finds Python 3.12 in this order: `--python PATH` argument, the `PYO3_PYTHON` environment variable, `uv python find 3.12`, then `python3.12` on `PATH`. `--venv PATH` builds the environment somewhere other than the repository root, which is how an installed copy gets its own. It installs the pinned versions from `requirements.txt` (`build123d==0.11.1`, `cadquery-ocp-novtk==7.9.3.1.1`) and verifies that `build123d` and `OCP` import.

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

## Test runner

`just test` runs `cargo test --workspace`. With `CADMARK_TEST_RUNNER=nextest` it runs cargo-nextest's `ci` profile instead (two retries, no fail-fast, a 120 s slow-test timeout that terminates a hung test after three periods, and a junit report at `target/nextest/ci/junit.xml`), then the doctests under plain `cargo test --doc`, which nextest cannot run. Any other value is an error.

The renderer's `picking_readback` integration test has its own `main` rather than libtest's, so it can point the GL loader at a software rasteriser before any driver opens. It answers the libtest command line both runners use: `--list --format terse` lists its checks, positional names select by substring (or by whole name with `--exact`), and `--nocapture` is accepted.

Source: `justfile`, `.config/nextest.toml`, `crates/cadmark-renderer/tests/picking_readback.rs`.

## Continuous integration

`.forgejo/workflows/ci.yml` runs `just verify` with `CADMARK_TEST_RUNNER=nextest` on every non-draft pull request and on pushes to `main` and `structural/**`, in the `forge-ci/rust` image on the Forgejo Actions runner. The image carries the Rust toolchain, cargo-nextest, just, and Mesa's Vulkan drivers (lavapipe serves as the software adapter); it has no Python 3.12, so the job installs a pinned uv, fetches the pinned CPython 3.12 build, and runs `just bootstrap` against it, caching the interpreter and `.venv` on the pins and `requirements.txt`. A pull request whose head commit already has a verdict from the workflow is not re-run. A test that failed and then passed on retry marks the PR with the `Flaky Test` label.

Source: `.forgejo/workflows/ci.yml`.

## Desktop installation

`just install` builds the workspace in release mode and runs
`scripts/install-desktop-app`, which lays out a copy independent of the
checkout under `$CADMARK_PREFIX` (default `~/.local`):

| Path | Contents |
|------|----------|
| `lib/cadmark/bin/` | `cadmark` and `cadmark-kernel-worker` side by side — the application starts the worker from its own directory |
| `lib/cadmark/.venv/` | a Python runtime built from the same interpreter the binaries were compiled against |
| `bin/cadmark` | wrapper that exports `VIRTUAL_ENV` for the installed runtime, then execs the application |
| `share/applications/cadmark.desktop` | menu entry, with `Exec` and `Icon` rewritten to absolute paths |
| `share/icons/hicolor/scalable/apps/cadmark.svg` | application icon |

The wrapper's `VIRTUAL_ENV` is what makes the installed copy use its own
runtime: `discover_venv` checks that variable first, then a `.venv` beside
the executable or up to three directories above it, then the workspace root
baked in at compile time, then the working directory — so an installed copy
started without the wrapper still finds `lib/cadmark/.venv` before the
checkout it was built from. `WorkerLaunch::beside_current_exe` resolves the
environment in the parent and passes it to the worker as `--venv`, because the
worker's own environment is cleared before it starts.

The script finishes by executing a test model through the installed worker, so
a broken runtime fails the install rather than the first build.
`scripts/install-desktop-app --uninstall` removes everything in the table.

Source: `justfile`, `scripts/install-desktop-app`, `assets/cadmark.desktop`, `assets/cadmark.svg`, `crates/cadmark-kernel/src/execution.rs`, `crates/cadmark-kernel/src/worker.rs`.
