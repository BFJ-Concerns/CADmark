# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added

- The AI can point back at geometry: its reply names faces, edges and
  vertices in square brackets and exactly those elements are highlighted,
  in their own colour, distinct from the user's selection. Faces and edges
  light up in the viewport; the viewport draws no vertex markers yet, so a
  vertex reference is understood but not yet visible. Every run tells the
  AI what it may name; on a model too large to list element by element it
  can ask for the detail of any run of elements, so the precision holds
  whatever the model's size.
- Project menu in the toolbar: shows the open folder, opens another
  through the system folder picker, lists recent projects, and opens
  `part.py` or the folder externally
- Named versions: Ctrl+S or the history menu records the current script
  as a design step with a name of your choosing
- History menu listing every design step with the current one marked;
  undo and redo tooltips say what they will restore
- Read-only code panel (Code button, Ctrl+E) showing the executed script
  with line numbers, the selected element's source line highlighted, and
  a warning when the file has changed on disk
- Keyboard shortcuts: Ctrl+Z/Ctrl+Shift+Z undo and redo, Ctrl+O open
  project, F5 rebuild, F fit view
- Status bar shows the model's measurements, the current selection and
  the navigation controls

### Changed
- Coherent dark theme shared by every panel: one accent colour for
  selection in the viewport and spatial comments in chat, three text
  levels, card-based messages
- Chat messages are told apart by position and colour rather than
  labels; errors and availability notices are distinct from AI replies
- Chat input is multi-line: Enter sends, Shift+Enter breaks the line
- Comment overlay stays within the viewport, submits on Enter, and
  shows a key hint instead of separate Submit and Cancel buttons
- Window title carries the project name

### Fixed
- Clicking empty viewport space could select a phantom vertex: the
  picking texture now clears every channel to zero
- Chat panel grew wider on every frame when it held a message

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
