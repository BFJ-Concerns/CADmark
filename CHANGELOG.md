# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.2.1] - 2026-09-24

### Added
- Each part of a multi-part model is drawn in its own colour, from a fixed
  palette by the part's position in the script; a single-part model keeps
  its grey
- A Parts tab in the left panel, beside Parameters: every part with its
  colour swatch, name and measurements, the active one highlighted; clicking
  a name selects the whole part, and a tick box hides or shows it in the
  viewport. A hidden part is neither drawn nor clickable, and stays hidden by
  name through rebuilds
- Every AI request is recorded in the project folder under
  `.cadmark/requests/`, one file per model call: the request as sent
  (images summarised), each streamed event with its offset, and the outcome
  with the token counts the provider reported. Files are written as the
  stream arrives, so a call still running can be read; the newest sixty are
  kept
- The chat's context figure gains the provider's own counts for the last
  request, on a line under CADmark's estimate: tokens read, how many the provider's
  cache served, tokens written, and how many of those were reasoning
- A reasoning-effort setting, sent as the request's `reasoning.effort`
  when filled in; blank leaves the provider's default
- The installed application writes its log to
  `~/.local/state/cadmark/cadmark.log` (under `XDG_STATE_HOME` when set),
  one line per AI request start and end among the rest
- The AI's thinking shows in the chat while a turn runs: "thinking…" as
  it reasons before each reply, then "thought for 2m 05s" once it speaks
  or calls a tool, opening to the reasoning text where the provider shares
  any. Reasoning counts as activity, so a long think no longer reads as a
  quiet stream, and a turn that ends during one records how long it was

### Changed
- Clicking any part of a multi-part model selects that part's face, edge or
  vertex; before, only the part the script bound last answered a click, and
  the others needed "Pick part" first
- Alt+click selects the whole part under the cursor; the toolbar's
  "Pick part" button, which armed the next click for that, is gone
- Two comment anchors on different parts are no longer offered as a
  distance measurement, since each part numbers its elements on its own
- Model sessions preserve provider output in order, including encrypted
  reasoning, assistant message metadata, original tool arguments and renders.
  Tool results follow the complete response; later turns append context to the
  saved history, including after reopening, permitting prompt-cache reuse
- Turn-scoped skill instructions are appended without changing the system
  instructions or the existing conversation prefix
- Output-limited replies can continue from complete encrypted reasoning even
  without visible text; missing usage details read "not reported" rather than zero
- Changing the endpoint, model or image capability rebuilds history from chat
  instead of sending opaque reasoning to a different backend; interrupted tool
  calls receive a result before the model session is saved
- The AI is asked to work in small steps — one change, one run, one look
  at the result — rather than one response that rewrites everything
- The AI says what it is about to do before each tool call, or each run
  of related calls, and what a result changed when it matters, so the
  text between the tool lines reads as a running commentary rather than
  a single line per reply
- Every dependency is refreshed to its newest compatible release; the
  wgpu, egui, and PyO3 majors stay where they are

### Fixed
- Exporting one part from the toolbar's per-part menu wrote nothing and
  reported the part as no longer existing: the menu named parts by their
  picking ID where the export looked them up by ordinal
- The model's context window is detected through gateways that report it
  only in Anthropic's model-list format, such as CLIProxyAPI; before, such an
  endpoint read as reporting nothing and the manual setting stood in

## [0.2.0] - 2026-09-23

### Added
- Continuous integration on Forgejo Actions runs `just verify` on every pull
  request and lane push; `CADMARK_TEST_RUNNER=nextest` selects cargo-nextest's
  retrying profile so a flaky test is labelled rather than failing the run
- The AI edits the script in place rather than rewriting it: `edit_script`
  replaces exact text and reports the edited region with line numbers,
  `read_script` shows lines by number, and `run_script` without `code` runs
  the file as edited. A whole-file `run_script` remains for a new part or a
  rewrite. Edits never followed by a successful run are discarded and the
  turn reported as failed
- `run_python` runs scratch Python after the current script, in its
  namespace, inside the confined kernel worker under the script limits, and
  returns what it printed, the value of its last expression, and any
  traceback, so the AI can measure and inspect geometry before changing the
  script
- What a script prints comes back to the AI with the run result, and with
  the traceback when the script fails
- Images attach to chat messages: the Attach… button, Ctrl+V with an image
  on the clipboard, or files dropped on the window stage removable
  thumbnails above the input, and a message may be an image alone. Each
  image is stored once under `.cadmark/attachments/` and rides with its
  message whenever the AI reads the history, so a picture from an earlier
  turn stays in front of the model. The project's `references/` folder is
  an AI-maintained library for pictures worth keeping beyond one
  conversation, catalogued in `references/INDEX.md`

### Changed
- While a turn runs, each tool call shows as its own collapsed line in the
  chat, so calls can be watched as they arrive; when the turn ends each run
  of calls folds into one line counting them ("8 tool calls: ran the script
  ×6, looked up docs ×2"), while the AI's text between them stays shown
- Script execution no longer spends most of its time in CADmark's own
  bookkeeping: a moderately complex part that took over two minutes now
  builds in about the time build123d itself needs. The provenance
  instrumentation looks moved copies of a shape up by hash instead of
  scanning every recorded shape, reads the kernel's history lists without
  triggering a C++ exception per query, and the exact bounding box is
  searched only on the faces and edges that can extend it. The execution
  log now states how long the script and each capture stage took
- Version numbers follow semantic versioning in place of the earlier
  date-derived scheme

### Fixed
- What the AI wrote before a turn failed or was cancelled stays in the chat
  instead of disappearing with the turn
- A reply the provider cuts off at its output-token limit is no longer
  reported as "provider request failed" and thrown away: CADmark keeps the
  text, runs the tool calls written in full, says in the chat that the reply
  was cut off, and asks the AI to continue
- Errors a provider sends partway through a reply are reported by cause
  (usage limit, overload, credential) with their real message, rather than
  as "type error"; reply text a provider sends only at the end of an item
  is no longer lost
- A long parameter name or expression no longer widens the parameters panel
  past its column, which left a dark void between the panel and the viewport;
  the row is cut short and the full text sits in its tooltip

## [0.1.0] - 2026-09-19

### Added
- A sketch exports as a drawing: SVG and DXF written flat in the plane the
  sketch was drawn on at true size, or STEP carrying its faces and curves.
  The Export menu offers the formats the result on screen can take
- A selected sketch curve shows its length or diameter and a region its
  area, and the status line gives the sketch's overall size in its plane;
  the AI receives the same measurements with a sketch anchor
- The comment overlay names the line that drew a clicked sketch curve,
  corner or region, and on a solid lists the sketch line beneath a face or
  edge that came from one; hovering that row brings the line into the code
  panel. Where a sketch operation rebuilt the outline, the operation is
  named as the reason no drawn curve survives
- build123d's 2D chamfer and offset keep each curve's drawing line through
  the kernel's own history; 2D fillet, make_face and make_hull are recorded
  as operations and state themselves as the barrier
- Edges and vertices are clickable: edges are drawn at a width you can
  hit and every vertex gets a marker, both keeping their size on screen
  however far you zoom
- Select menu in the toolbar turns face, edge or vertex clicks off, so a
  click passes through to what is behind; all three start on
- Parameters panel listing every module-level numeric name in the open
  part's script: editing a value rewrites that one number, rebuilds the
  model and records a design step, with no AI turn; names derived from
  other parameters are shown as their expression
- Ambiguous anchors offer their candidate source lines in the comment
  overlay, most likely first where the ledger can rank them and plainly
  unordered where it cannot; hovering a candidate highlights its own line
  in the code panel and the geometry that line accounts for in the
  viewport; choosing one sends that line alone to the AI, and choosing
  none sends them all as before
- Selecting a face or edge shows which sketch curve produced it, and a sketch
  curve, corner or region can be clicked and commented on like any other
  selection, anchored to the line that drew it. Where the
  kernel keeps no route from the sketch to that element — across a clean-up
  step, or a boolean's edge inputs — the reason is stated rather than a
  nearest plausible line being offered
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
- Multi-part scripts render every distinct completed `BuildPart` and top-level
  `Part`, `Solid`, or `Compound` binding; aliases of one shape are de-duplicated.
  Rebind one name when constructing a part in stages. They provide a dedicated
  whole-part picker and export one selected part or all parts from retained
  worker BREP files
- A design that has reached only a sketch renders: its curves, corners and
  enclosed regions draw face-on to the sketch's plane in an orthographic
  view, with any solid already on screen ghosted behind them. A sketch-only
  script is a successful build rather than an error, and the status line,
  the chat and the AI's render all report it as a profile that is not yet a
  solid, with no volume or validity to report
- Section plane: cut the model along X, Y or Z, slide the plane across
  it, and flip which half is kept, so an internal pocket can be seen
  without exporting. Cut-away geometry is also unclickable
- Ghost button makes the model see-through, another way to see inside
  it without cutting

### Changed
- The top-level Export entries of a multi-part model write the part
  currently selected and name it in the menu, rather than the last part
  the script bound
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
- The AI is sent the script on disk with every turn, so a reopened project,
  a new conversation or a condensed chat no longer leaves it reconstructing
  the file from memory or asking for it to be pasted; when the file differs
  from the AI's last run, a value edited in the parameters panel or a step
  undone, it is told which parameter values changed and keeps them
- Malformed settings errors identify the affected file and repair location
  without echoing settings content
- Completed-turn chat reports now show face count, volume, and overall size
  before and after an unchanged edit
- History now lists every CADmark design step, including abandoned alternatives,
  without navigating them as undo and redo steps
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
