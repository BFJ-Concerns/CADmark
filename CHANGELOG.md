# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Start view on launch: recent projects, opening a project folder, and
  creating a new one. Nothing is loaded until a project is chosen; a folder
  named on the command line still opens directly. Chat stays available there:
  a message typed before a project is open is sent as the first turn of the
  project you then choose
- A project folder holds any number of parts. The toolbar's part menu
  switches between them and creates a new one; a new part is Untitled until
  the first save asks for its name, which becomes its file name, after which
  the same key names a version
- Design steps record which part they changed, so undo reopens that part
- A design that has reached only a sketch renders: its curves, corners and
  enclosed regions draw face-on to the sketch's plane in an orthographic
  view, with any solid already on screen ghosted behind them. A sketch-only
  script is a successful build rather than an error, and the status line,
  the chat and the AI's render all report it as a profile that is not yet a
  solid and so cannot be exported or measured

### Changed
- New conversation control archives the current chat before starting a blank
  one, leaving the project script unchanged
- Chat shows estimated model-context occupancy, including reserved reference
  image budget, with the configured context-window setting
- Long conversations are condensed before approaching the configured context
  limit while retaining decisions and outstanding requests
- Spatial comments now give the AI surrounding script context and the selected
  element's adjoining final-topology elements
- Each AI request carries a curated library of worked build123d examples
  for the operations it names, so the model sees what a good script for
  that operation looks like as well as what the API does
- Per-part solid-validity status after execution; exports now stop with a
  non-blocking warning before writing an open or invalid part
- The AI can look at the model it just built: it renders the current
  shape offscreen — shaded, with edges, framed to the model, at the
  viewport's own resolution — from any standard view or your current
  camera, without disturbing what you see, including the model it built
  moments earlier in the same reply
- A model that cannot read images is told plainly that the render
  tool is unavailable, rather than left to infer its absence
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
- In-app readouts for a selected face's area, edge length or diameter, and
  the minimum distance between two selected elements

### Changed
- The AI names the sketch-or-solid route before it edits: when a change
  could be made to a sketch profile or to the solid, the reply says which
  one it is taking before the script runs, and what it said stays in the
  chat above the tool calls
- Spatial comments stay as editable, removable pending cards and send together
  with chat text as one turn; their viewport highlights share each card's colour
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
- Completed-turn chat reports now show face count, volume, and overall size
  before and after an unchanged edit
- Repeated edits from the same undone design step now create separate history
  alternatives instead of colliding with the earlier edit
- The project-folder picker is parented to the CADmark window on Wayland
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
