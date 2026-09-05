# CADmark task runner. `just` with no argument lists every recipe.

# Where `just install` puts the application. Override with CADMARK_PREFIX.
prefix := env("CADMARK_PREFIX", env("HOME") / ".local")

default:
    @just --list

# --- Python runtime -------------------------------------------------------

# Create the repository .venv holding build123d and OCP.
bootstrap *ARGS:
    scripts/bootstrap-python-runtime {{ ARGS }}

# Fail with an actionable message when the kernel has no Python runtime.
[private]
require-venv:
    #!/bin/sh
    [ -x .venv/bin/python ] && exit 0
    echo "No .venv — run 'just bootstrap' first." >&2
    exit 1

# --- Development ----------------------------------------------------------

# Type-check every crate, including tests and benches.
check:
    cargo check --workspace --all-targets

# Format the workspace in place.
fmt:
    cargo fmt --all

# Fail if anything is unformatted.
fmt-check:
    cargo fmt --all -- --check

# Lint with warnings escalated to errors.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run the whole test suite. Extra arguments reach cargo test.
test *ARGS: require-venv
    cargo test --workspace {{ ARGS }}

# Run the tests of one crate, e.g. `just test-crate cadmark-kernel`.
test-crate CRATE *ARGS: require-venv
    cargo test -p {{ CRATE }} {{ ARGS }}

# Everything CI would check: formatting, lints, tests.
verify: fmt-check lint test

# Build and run the app. `just run mypart` opens a project folder.
run *ARGS: require-venv
    cargo run --bin cadmark -- {{ ARGS }}

# Optimised build of every binary.
build:
    cargo build --workspace --release

# Remove build artefacts.
clean:
    cargo clean

# --- Installation ---------------------------------------------------------

# Build the current tree and install it as a desktop application.
install: require-venv build
    scripts/install-desktop-app --prefix '{{ prefix }}'

# Remove the installed application, its runtime, and its launcher entry.
uninstall:
    scripts/install-desktop-app --prefix '{{ prefix }}' --uninstall
