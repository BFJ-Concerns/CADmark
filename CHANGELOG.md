# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.2603.3100] - 2026-03-31

### Added
- Rust workspace with six crates (core, kernel, renderer, UI, bridge, app)
- Provenance ledger mapping rendered geometry to generating build123d code
- Geometry context with pluggable identification strategies (ADR-0003)
- Spatial comment system with overlay, anchor line, and applied-state lifecycle
- Chat pane with three visually distinct message types
- wgpu renderer with shaded mesh, wireframe overlay, and selection glow
- GPU colour-ID picking for face, edge, and vertex selection
- Offscreen depth pipeline with blit compositing into egui viewport
- Camera with orbit, pan, and zoom (right-drag, middle-drag, scroll)
- Undo/redo via git microversions with toolbar dropdown
- Claude Code subprocess as AI backend behind replaceable AiBackend trait
- Orchestrator with async tokio channels between UI and AI bridge
- Git operations: microversion commits, named snapshots, branch alternatives
- PyO3 bridge to build123d with OCP builder instrumentation for provenance
- Tessellation extraction via BRepMesh for renderer consumption
