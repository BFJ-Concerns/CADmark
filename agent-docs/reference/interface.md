---
description: Viewport, toolbar, chat pane, panels, and all interactive controls
---

# Interface reference

## Start view

Shown on launch when no directory argument is given. Offers three actions: "New project" (choose an empty folder), "Open project folder" (system folder picker), and a list of recent projects. A settings button opens the settings dialog from here.

Nothing is loaded until a project is chosen. A message typed before choosing a project becomes the first turn of whichever project is then opened.

Source: `crates/cadmark-ui/src/start_view.rs`.

## Projects and parts

A project is a folder. A folder holds any number of parts as Python scripts. The toolbar's part menu lists every part, offers "New part", and switches between them.

A new part is "Untitled" until its first save (`Ctrl+S`), which asks for a name that becomes the file name. After naming, the same shortcut records a named version.

Window title carries the project folder name. The project menu shows the folder path, offers "Open project folder" (`Ctrl+O`), "Open recent", opening the script in an external editor, and showing the folder in the file manager.

Each open project owns one orchestrator thread (`OrchestratorHandle`: command sender, result receiver, join handle). Opening another project and closing the window both call `Project::shut_down`, which cancels any running turn, closes the command channel so the thread's loop ends, drains results, and joins the thread before the window's GPU device is torn down. After shutdown the project sends nothing and polls nothing.

Source: `crates/cadmark-ui/src/toolbar.rs`, `crates/cadmark-app/src/project.rs` (`shut_down`), `crates/cadmark-app/src/app.rs` (`on_exit`, `open_project`).

## Viewport

3D viewport rendered with wgpu. Navigation:

| Control | Action |
|---------|--------|
| Right-drag | Orbit |
| Middle-drag | Pan |
| Scroll | Zoom |
| Click | Select geometry / place comment |

Curved surfaces and edge polylines use a display tolerance relative to model size. This approximation affects the viewport mesh; the kernel retains exact CAD geometry for modelling and STEP export.

Source: `crates/cadmark-kernel/src/tessellation.rs` (`prepare_display_tessellation`).

The world is Z-up (build123d convention). The status bar shows navigation controls as a hint.

Source: `crates/cadmark-renderer/src/camera.rs:1–11`, `crates/cadmark-ui/src/status.rs:96–99`.

## Selection

Clicking geometry selects it: faces, edges, vertices. Edges and vertex markers have smooth, fine outlines and wider invisible hit targets. Both visible and picking sizes remain constant in screen pixels at any zoom. "Pick part" in the toolbar selects a whole part with the next click.

Sketch-only designs display curves, corners and filled regions face-on to the sketch plane in orthographic view, with any existing solid ghosted behind them. Each element carries the kernel's exact measurement (`SketchCurve::{curve_type, length, radius}`, `SketchRegion::area`), which the status bar reads through `SketchProfile::measurement` and the AI receives through `SketchProfile::identification`. The sketch lineage ledger of a sketch result is keyed by the same region, curve and corner IDs the profile draws under (`SketchLineageLedger::lookup_element`), built by `finalise_sketch` from the placed shape the profile was extracted from; build123d's 2D `chamfer` and `offset` report per-edge maker history and keep each curve's drawing line, while `fillet`, `make_face` and `make_hull` rebuild the outline and are recorded as the barrier the route could not cross. The three kinds are pick targets in the colour-ID pass, drawn after the solid passes with depth ignored (as the visible profile is): regions through the face vertex stage, curves as edge-width quads, corners as vertex-marker discs (`render_sketch_picking` in `crates/cadmark-renderer/src/viewport.rs`, `sketch_*_pick_vertices` in `pipeline.rs`). Their IDs occupy the sketch ranges of `picking.rs` and decode to `PickedElement::Sketch`. The filter maps Faces/Edges/Vertices to regions/curves/corners (`SelectionFilter::allows_pick`). The ghosted solid behind a sketch is excluded from the picking pass, since its ledger belongs to an earlier script. The visible sketch shader tints the selected and hovered element with the same colours the mesh pass uses.

Selected elements glow in the viewport. The status bar shows which element is selected. A vertex marker on geometry the section plane has cut away is neither drawn nor pickable.

Source: `crates/cadmark-renderer/src/markers.rs` (dimensions), `crates/cadmark-renderer/src/shaders/wireframe.wgsl` and `crates/cadmark-renderer/src/shaders/vertex_markers.wgsl` (coverage).

### Select menu

The "Select" menu in the toolbar controls which element kinds a viewport click can land on. Three checkboxes — "Faces", "Edges", "Vertices" — all on by default. Disabling a kind makes the click pass through it to whatever is behind. The button label reads "Select" when all kinds are on, or "Select: edges, vertices" (for example) when one is off.

The filter does not affect whole-part picks.

Source: `crates/cadmark-ui/src/toolbar.rs` (menu UI), `crates/cadmark-renderer/src/picking.rs:38–63` (`SelectionFilter`), `crates/cadmark-renderer/src/shaders/vertex_markers.wgsl:105–109` (section discard).

## View cube

A labelled cube in the viewport's top-right corner, drawn only when geometry is loaded. It turns with the camera. Each visible face is divided into nine click regions: the centre looks square at that face (six axis views), an edge strip looks from the 45° between two faces (twelve views), a corner looks from the three-way diagonal (eight isometric views). Hovered regions highlight on every face sharing them; a tooltip names the view ("Look from the front-top-right"). Primary drag on the cube orbits. Two curved arrows in the canvas's top corners, shown on hover, roll the view ±90° about the line of sight; any snap to a named view resets the roll to zero.

A row of small buttons beneath: "Fit" (frames the model; `F`), the projection toggle labelled with the current mode, "Persp" or "Ortho" (`P`), and "XYZ" (toggles an axis triad in the canvas's bottom-left; off by default, persisted in user settings as `show_axes`).

The camera's orbit distance ranges from 0.001 to 10,000,000 units. Clip planes are derived per frame from the framed model's bounding sphere and the eye position, so the model is never cut by the near or far plane; the near plane stays at least 1/10,000 of the far plane for depth precision.

Source: `crates/cadmark-ui/src/view_cube.rs` (cube, regions, arrows, triad), `crates/cadmark-renderer/src/camera.rs` (`roll_quarter_turn`, `clip_planes`), `crates/cadmark-app/src/app.rs` (`apply_view_action`).

## Section plane

Cuts the model along X, Y, or Z. Toggled with the "Section" button in the toolbar. When active, the toolbar shows:

- Axis buttons (X, Y, Z) to choose the cut direction
- A slider to position the plane along the chosen axis, bounded by the model's extent
- A flip button to keep the other half

Cut-away geometry is also unclickable: every picking pass, including the exact depth prepass (`fs_depth` in `picking.wgsl`), discards fragments the plane cuts away, so a click on the cut reaches the interior face it reveals rather than being blocked by the removed geometry's depth.

Source: `crates/cadmark-ui/src/toolbar.rs`, `crates/cadmark-renderer/src/shaders/picking.wgsl`.

## Ghost mode

The "Ghost" button makes the model see-through. Another way to inspect internal geometry without cutting.

Source: `crates/cadmark-ui/src/toolbar.rs`.

## Chat pane

Conversation with the AI. Message types, distinguished by position and colour:

- User messages (right-leaning card)
- Spatial comments (accent-tinted, with chips naming each anchor's element and source line)
- AI replies (plain card, streamed as the turn progresses). The system prompt asks the model to say what it is about to do before each tool call or run of related calls, and what a result changed when it matters, so a turn carries a short reply before most tool runs. A reply of several message items shows them separated by a blank line; text a provider sends only in a completed item, not as deltas, still appears
- Tool calls. While the turn runs, each call is its own collapsed line ("running the script…" until its result arrives), expandable to its input and result; a call the turn never finished reads "did not finish"
- Thinking. The model's reasoning before each reply is one record: "thinking…" while it streams, "thought for 2m 05s" once the model speaks, calls a tool, or the turn ends; a record with no end after the turn reads "did not finish". Its text is whatever reasoning the provider shares (`response.reasoning_summary_text.delta` / `response.reasoning_text.delta` on the stream; a gateway relaying Claude sends empty deltas), shown under the line when there is any. Every reasoning delta, empty or not, counts as a turn event for the phase line's quiet time. Thinking is never replayed to the model and occupies no context
- Once the turn ends, each run of consecutive steps — tool calls and thinking — folds into one collapsed line, "N tool calls: ran the script ×2, looked up docs · thought for 1m 20s", which opens to the per-step lines; a run of thinking alone stays as its own lines. The AI speaking, or a CADmark notice, starts a new run
- Notices from CADmark (quiet, or red on failure; not shown to the AI)
- Design changes made outside the chat (quiet; sent to the AI as a "Note from CADmark" user item in history order): a parameter set in the panel, a design step undone, redone, or jumped to

While a turn runs: a phase line showing the current step, elapsed time, time since last event, and a Cancel button.

Text the AI wrote stays in the chat when the turn fails or is cancelled, above the notice saying how it ended; only a reply with no text is removed. A reply the provider cuts off at its output-token limit (`response.incomplete` with reason `max_output_tokens`) is kept: its fully written tool calls run, a call cut off mid-arguments is dropped, a notice says the reply was cut off, and the AI is asked to continue. A cut-off reply with no text and no complete tool call fails the turn, naming the output limit. Any other incomplete reason fails the turn naming that reason. An error event inside an accepted stream is reported by cause (usage limit, overloaded, credential, unknown model) as a refused request would be.

Input: multi-line text field. Enter sends, Shift+Enter breaks the line.

Context occupancy shown in the chat against the configured context-window setting. The figure is the estimated weight of the next request as it would be assembled now (`RequestAssembly` in `crates/cadmark-app/src/turn.rs`): the conversation, the reserved reference-image budget, and the request's own overhead — system instructions with any active skill, tool definitions, the `<current_script>` block, the selected examples, and the draft chat text and pending comments. Hovering shows the breakdown. Condensation triggers at three quarters of the window when the conversation is at least a tenth of it; when the request alone is at three quarters the figure turns amber with a warning, since condensing cannot help.

The AI's tools (`crates/cadmark-bridge/src/tools.rs`, dispatched in `TurnRunner::run_tool` in `crates/cadmark-app/src/turn.rs`):

| Tool | What it does |
|------|--------------|
| `run_script` | Executes the script and rebuilds the model. With `code` it replaces the whole file first; without `code` it runs the file as `edit_script` left it. The result carries measurements, validity, and what the script printed, or the traceback (with anything printed before the failure). `summary` names the design step. |
| `edit_script` | Replaces one exact occurrence of `old_text` with `new_text` (every occurrence with `replace_all`); refused when the text is absent or ambiguous. Returns the edited region with line numbers. Nothing runs. |
| `read_script` | The script, or a line range, with line numbers. |
| `run_python` | Runs a snippet after the current script in its namespace (or alone with `standalone`) under the script execution limits, in the confined kernel worker; returns what it printed, the `repr` of a trailing expression, and any traceback. Keeps no model. |
| `lookup_docs` | Answers a build123d question from the bundled documentation. |
| `render_view`, `reference_images`, `keep_reference` | Image tools, offered only to a model that reads images. |

A turn keeps only its last successful `run_script`: edits with no successful run after them are discarded and the turn is reported as failed with the script restored. The kernel worker serves snippets through `WorkerRequest::RunSnippet` (`crates/cadmark-kernel/src/protocol.rs`, `execution::run_snippet`), and captures a script's stdout into `ExecutedModel::printed`, cut at `PRINTED_OUTPUT_LIMIT`.

Every request carries the script on disk in a `<current_script>` block placed before the user's words, whether or not a skill is active. The block states whether the file is unchanged since the last successful `run_script` in the saved conversation (the executed text is recorded on the tool call as `executed_source`, since a run after edits carries no `code`), differs from it (naming each parameter whose literal value changed), or has no run in the conversation at all (a new or condensed chat). An empty part is stated as having no script yet.

A completed turn's reply ends with a change report: face count, volume, and overall size before and after the turn. A single-part model reads as "Model change: …" or "Model unchanged." with both sets of values. A multi-part model reports one line per part under its script binding name, matched to the part of the same name before the turn; a binding no longer produced is "removed", one not produced before is "new". The `run_script` tool result the AI reads carries the same per-part measurements ("Parts: name: …; name: …") when the script completed more than one part.

Source: `crates/cadmark-ui/src/chat.rs` (`show_tool_calls`, `tool_group_label`, `tool_call_label`), `crates/cadmark-app/src/app.rs` (`finish_turn`, `record_tool_start`), `crates/cadmark-bridge/src/openai_compatible.rs` (`ResponseAssembly`), `crates/cadmark-app/src/turn.rs` (`current_script_block`, `last_successful_run`, `describe_model`), `crates/cadmark-core/src/geometry.rs` (`describe_model_change`).

The **Skills** menu inserts a built-in command into the draft. Start a message or
spatial comment with `/3d-printing` or `$3d-printing` to apply printing guidance
to that turn; active skills are shown beside the menu. See the
[printing skill guide](../guides/3d-printing.md).

## Spatial comments

Click geometry to open the comment overlay near the selection point. Type free text and press Enter to submit; Escape cancels. Clicking more geometry while the overlay is open adds anchors — one comment can reference several elements.

Comments stay as editable, removable pending cards and send together with optional chat text as one turn. Their viewport highlights share each card's colour.

For ambiguous anchors (geometry traceable to multiple source lines), the overlay lists candidate lines — ranked by likelihood where the ledger can, and in recorded order where it cannot. Hovering a candidate highlights its line in the code panel and the geometry that line accounts for in the viewport. Choosing a candidate sends that line alone to the AI; choosing none sends all candidates.

Source: `crates/cadmark-ui/src/overlay.rs`, `crates/cadmark-ui/src/chat.rs:60–73`.

## Parameters panel

Lists every module-level numeric name in the open part's script. Names bound to a literal show a drag/type field; names derived from other parameters show their expression and are read-only.

Editing a value rewrites that one number in the script, rebuilds the model, and records a design step. No AI turn is involved.

Source: `crates/cadmark-ui/src/parameters.rs`, `crates/cadmark-app/src/script_parameters.rs:1–53`.

## Code panel

Read-only view of the executed build123d script with line numbers. Toggled with the "Code" button or `Ctrl+E`. Shows:

- The selected element's source line highlighted
- A warning when the file has changed on disk since last execution
- Buttons to copy the source and open it in an external editor
- A Refresh button (also `F5`) to re-execute after external edits

Source: `crates/cadmark-ui/src/code_panel.rs`.

## Status bar

Bottom of the window. From left to right:

- Activity indicator (spinner + phase text) while a build or turn runs, else the last outcome
- Model measurements: face count, volume, bounding box (from `ModelSummary`)
- Current selection label
- Selection measurement: face area (mm²), edge length (mm), or circular-edge diameter (mm)
- Navigation hint: "Right-drag orbit · Middle-drag pan · Scroll zoom · Click to comment"

Source: `crates/cadmark-ui/src/status.rs`.

## In-app measurement

Automatic for selected elements:

| Element | Measurement |
|---------|-------------|
| Face | Area in mm² |
| Straight edge | Length in mm |
| Circular edge or arc | Diameter in mm (from radius, not arc length) |
| Two selected elements | Minimum distance in mm (computed by the kernel) |

Source: `crates/cadmark-ui/src/status.rs:46–65`, `crates/cadmark-kernel/src/measurement.rs`.

## Design history

Every accepted edit is recorded as a design step. Steps record which part they changed, so undo reopens that part.

- Undo: `Ctrl+Z` / toolbar button
- Redo: `Ctrl+Shift+Z` / toolbar button
- Jump to step: click in the History menu (newest first, current marked)
- Name a version: `Ctrl+S` / "Name this version" in the History menu
- "New conversation" at the top of the chat pane archives the chat and starts a blank one; the script is unchanged. It is disabled while a build or AI turn runs.

Named versions are highlighted in the history menu. Undo and redo tooltips describe what they will restore.

Source: `crates/cadmark-ui/src/toolbar.rs` (history controls), `crates/cadmark-app/src/app.rs` (`CadmarkApp::show_chat`).

## Export

Export menu in the toolbar. Available when a model is loaded. The formats offered depend on what the script produced (`LoadedModel::export_formats`):

| Result | Format | Extension | Description |
|--------|--------|-----------|-------------|
| Solid | STEP | `.step` | STEP AP214 B-rep for other CAD tools |
| Solid | STL | `.stl` | Binary STL mesh for slicers |
| Solid | 3MF | `.3mf` | 3MF mesh with units for slicers |
| Sketch | SVG | `.svg` | Vector drawing in the sketch's plane, millimetres |
| Sketch | DXF | `.dxf` | 2D CAD drawing in the sketch's plane |
| Sketch | STEP | `.step` | The sketch's faces and curves as B-rep |

Export writes the file next to the part script from the BREP the worker retained for the result, without re-running the script. A drawing format carries the sketch's plane (`SketchPlane`) across the worker boundary and flattens the shape onto it with build123d's `Plane.to_local_coords` before `ExportSVG` or `ExportDXF` writes it. Multi-part models offer per-part and "Export all" options; the top-level entries write the selected part (`Project::request_export` routes to the active part when several exist) and the menu names it. An open or invalid solid shows a non-blocking warning before export.

Source: `crates/cadmark-core/src/export.rs`, `crates/cadmark-kernel/src/export.rs`, `crates/cadmark-app/src/project.rs`, `crates/cadmark-ui/src/toolbar.rs`.

## Solid validity

After each build, the status bar reports every produced part's validity. The message comes from `describe_validity`:

- "Part 1 is closed and valid." — a printable solid
- "Part 1 is NOT a closed valid solid; it will not print." — open or invalid geometry

When exporting, an invalid part shows a non-blocking warning: "Cannot export: Part N is not a closed valid solid."

Source: `crates/cadmark-app/src/validity.rs:33–53`, `crates/cadmark-app/src/app.rs:886–896`.

## Images in chat

Images are attached to a message, not to the project. Three routes stage an image in the chat input: the **Attach…** button (a multi-file picker offering every file, since the format is read from the bytes rather than the name), `Ctrl+V` in the input when the clipboard holds an image rather than text, and dropping files on the window. Staged images show as removable thumbnails above the input; a message may be an image alone. PNG and JPEG are accepted, up to 20 MB each; anything else is refused with the reason in the status bar.

On send, each staged image's original bytes are written once to `.cadmark/attachments/<timestamp>-<name>.<ext>` and the message records the file name, the user-facing name, and the media type; the conversation file never carries image bytes. Reopening the project reloads the bytes by file name; a missing file leaves the message showing the name with no image to send. The sent message shows thumbnails.

When `ai.accepts_images` is enabled, the message's images ride with it on the wire every time the message is replayed to the model, and each is reserved at a fixed 765-token budget in the context figure. Whether or not it is enabled, each user message ends with an `[Attached images: …]` line naming each image and, in brackets, its stored file name, which is unique and is how `keep_reference` identifies an attachment; when images are disabled that line is all the model gets.

### The reference library

`references/` in the project folder is the AI-maintained library, shared by every conversation, with `references/INDEX.md` as its catalogue (one `- \`file\`: description` line per image). Two tools, offered only to image-reading models, manage it: `reference_images` lists the library with each file's description (files placed by hand are listed without one) or returns one image by file name; `keep_reference` copies an attachment from any message in the conversation, identified by its stored file name, into the library under a chosen name and writes its description, or re-describes a file already there. The AI rewrites only the `- \`file\`: description` line it is updating (or appends one after the last such line); any other text in `INDEX.md` is the user's and is kept. Names are confined to the folder; the extension follows the image's format; an existing file is never overwritten.

Source: `crates/cadmark-app/src/reference_images.rs` (`StagedImage`, `store_attachment`, `load_attachment_bytes`, `ReferenceLibrary`, `ImagePicker`), `clipboard.rs`, `turn.rs` (`ReferenceSource`, `history_items`), `crates/cadmark-bridge/src/tools.rs`, and `crates/cadmark-ui/src/chat.rs` (`ChatPane::stage_image`, `show_attachments`).

## Keyboard shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z` | Redo |
| `Ctrl+O` | Open project folder |
| `Ctrl+S` | Name this version |
| `Ctrl+E` | Toggle code panel |
| `Ctrl+,` | Open settings |
| `F5` | Rebuild (re-execute script) |
| `F` | Fit view |
| `P` | Toggle perspective/orthographic |

All shortcuts are inactive while a text field has focus, except `Ctrl+E` and `Ctrl+,`. Undo, redo, open, save, and rebuild are also inactive while the AI is working or a dialog is open.

Source: `crates/cadmark-app/src/app.rs:1353–1403`.
