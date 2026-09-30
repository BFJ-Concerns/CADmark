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

Clicking geometry selects it: faces, edges, vertices. Edges and vertex markers have smooth, fine outlines and wider invisible hit targets. Both visible and picking sizes remain constant in screen pixels at any zoom. Every part of a multi-part model is pickable: a picking ID carries the element within its part in the low 20 bits and the part ordinal above (`crates/cadmark-renderer/src/picking.rs`, `Pick`), so one readback names both, and a click on a part other than the active one first makes it active (`Project::select_model_part`, swapping in its ledger, lineage and descriptors) before the element is resolved. A whole part is selected with Alt+click in the viewport (`PendingPick::whole_part` routes the click to the part-picking pass) or from the Parts tab. Each part is shaded from `PART_PALETTE` by ordinal.

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
- Thinking. The model's reasoning before each reply is one record: "thinking…" while it streams, "thought for 2m 05s" once the model speaks, calls a tool, or the turn ends; a record with no end after the turn reads "did not finish". Its text is whatever reasoning the provider shares (`response.reasoning_summary_text.delta` / `response.reasoning_text.delta` on the stream; a gateway relaying Claude sends empty deltas), shown under the line when there is any. Every reasoning delta, empty or not, counts as a turn event for the phase line's quiet time. The display record is not replayed as assistant text. Provider reasoning items, including encrypted continuation data, are retained separately in the model session and included in its context estimate
- Once the turn ends, each run of consecutive steps — tool calls and thinking — folds into one collapsed line, "N tool calls: ran the script ×2, looked up docs · thought for 1m 20s", which opens to the per-step lines; a run of thinking alone stays as its own lines. The AI speaking, or a CADmark notice, starts a new run
- Notices from CADmark (quiet, or red on failure; not shown to the AI)
- Design changes made outside the chat (quiet; sent to the AI as a "Note from CADmark" user item in history order): a parameter set in the panel, a design step undone, redone, or jumped to

While a turn runs: a phase line showing the current step, elapsed time, time since last event, and a Cancel button.

Text the AI wrote stays in the chat when the turn fails or is cancelled, above the notice saying how it ended; only a reply with no text is removed. A reply the provider cuts off at its output-token limit (`response.incomplete` with reason `max_output_tokens`) is kept: its fully written tool calls run, a call cut off mid-arguments is dropped, a notice says the reply was cut off, and the AI is asked to continue. A cut-off reply can also continue from a complete reasoning item with non-empty encrypted continuation data. With no text, complete tool call or resumable reasoning, it fails the turn naming the output limit. Any other incomplete reason fails the turn naming that reason. An error event inside an accepted stream is reported by cause (usage limit, overloaded, credential, unknown model) as a refused request would be.

Input: multi-line text field. Enter sends, Shift+Enter breaks the line.

Context occupancy shown in the chat against the configured context-window setting. The figure is the estimated weight of the next request as it would be assembled now (`RequestAssembly` in `crates/cadmark-app/src/turn.rs`): the conversation, the reserved reference-image budget, and the request's own overhead — system instructions, turn-scoped skill instructions, tool definitions, the `<current_script>` block, the selected examples, and the draft chat text and pending comments. Hovering shows the breakdown. Condensation triggers at three quarters of the window when the conversation is at least a tenth of it; when the request alone is at three quarters the figure turns amber with a warning, since condensing cannot help.

Beside the estimate, once a turn's request has completed, the provider's own counts for the last request of a turn: "Last request: N read (M cached) · O written (R reasoning)", read from the `usage` block of `response.completed` or `response.incomplete` (`input_tokens`, `input_tokens_details.cached_tokens`, `output_tokens`, `output_tokens_details.reasoning_tokens`; `crates/cadmark-bridge/src/openai_compatible.rs`, `read_usage`) and held on the project (`Project::last_usage`). Absent until a provider sends one. Missing cache or reasoning details are shown as "not reported", not zero. Output tokens include reasoning, but an empty visible reply does not establish how the limit was spent: an unfinished tool call can also consume it. Use the recorded events and reported usage to distinguish them.

The AI's tools (`crates/cadmark-bridge/src/tools.rs`, dispatched in `TurnRunner::run_tool` in `crates/cadmark-app/src/turn.rs`):

| Tool | What it does |
|------|--------------|
| `run_script` | Executes the script and rebuilds the model. With `code` it replaces the whole file first; without `code` it runs the file as `edit_script` left it. The result carries measurements, validity, and what the script printed, or the traceback (with anything printed before the failure). `summary` names the design step. |
| `edit_script` | Replaces one exact occurrence of `old_text` with `new_text` (every occurrence with `replace_all`); refused when the text is absent or ambiguous. Returns the edited region with line numbers. Nothing runs. |
| `read_script` | The script, or a line range, with line numbers. |
| `run_python` | Runs a snippet after the current script in its namespace (or alone with `standalone`) under the script execution limits, in the confined kernel worker; returns what it printed, the `repr` of a trailing expression, and any traceback. Keeps no model. |
| `lookup_docs` | Answers a build123d question from the bundled documentation. |
| `render_view` | A shaded PNG of the model with edges, at the viewport's resolution, from a standard view or the user's camera, framed to what it draws. Without `part` it draws every part the user has visible, each in its palette colour, and refuses when every part is hidden; with `part` it draws that one part alone, hidden or not, framed to it. A `part` that matches no name (exact first, then a unique case-insensitive match) is refused with the names that exist. Offered only to a model that reads images. |
| `check_export` | Proves whether exporting a part in a solid format would reproduce it: writes the file, reads it back, compares retained and written figures — solids and faces for STEP, closed shells for STL and 3MF, volume and size for both — and discards the file. The verdict and cause match what the user's own export would give: the faces the format lost (STEP only) with their surface and curve kinds, the discrepancies for a mesh, or the B-spline conversion STEP needed and the volume it moved. Accepts `format` (`step`, `stl`, `3mf`) and optional `part` (by name; the whole model when absent). |
| `reference_images`, `keep_reference` | Reference-library image tools, offered only to a model that reads images. |

A turn keeps only its last successful `run_script`: edits with no successful run after them are discarded and the turn is reported as failed with the script restored. The kernel worker serves snippets through `WorkerRequest::RunSnippet` (`crates/cadmark-kernel/src/protocol.rs`, `execution::run_snippet`), and captures a script's stdout into `ExecutedModel::printed`, cut at `PRINTED_OUTPUT_LIMIT`.

The model session preserves request items and provider output in order, including renders, reasoning items with their encrypted continuation data, assistant message metadata and the original tool-argument strings (`ModelSession` in `crates/cadmark-core/src/model_session.rs`). A complete response's output items precede its tool results. Calls interrupted before a result is available receive an interruption result before the session is saved. The chat's thinking record is a separate display record.

The next turn replays this sequence, including after reopening the project, then appends later chat messages, the current script, selected examples, turn-scoped skill instructions and the new input (`Conversation::replay`, `record_session_for`; `TurnEvent::ModelContext`). Activating a skill leaves the system instructions unchanged. This preserves the input prefix for cache reuse; actual cache hits depend on provider settings, routing and retention. New conversations and condensation start a fresh sequence. Changing the endpoint, model or image capability reconstructs history from the chat instead of replaying opaque output from the previous backend. Conversations saved without a model session also reconstruct history from chat.

Every request is recorded under `.cadmark/requests/` in the project folder as `<UTC timestamp>-<purpose>.jsonl`, purpose `turn`, `docs`, `condense`, or `probe` (`crates/cadmark-bridge/src/request_log.rs`; `AiServices::recording_to`, applied in `CadmarkApp` at project open). Lines, each one JSON object, flushed as written so a running call can be read: a `request` line with the body as sent (each image's data URL replaced by its type and size), one `event` line per streamed event with `at_ms` from the send, a `rejected` line with `http_status` and `body` when the provider refuses the request, and an `outcome` line with `events`, `outcome` (`completed`, `incomplete: max_output_tokens`, or `error: …`) and `usage` (null when the provider sent none). The newest sixty files are kept; the credential is never written. A record that cannot be written is logged and the request proceeds.

Every request carries the script on disk in a `<current_script>` block placed before the user's words, whether or not a skill is active. The block states whether the file is unchanged since the last successful `run_script` in the saved conversation (the executed text is recorded on the tool call as `executed_source`, since a run after edits carries no `code`), differs from it (naming each parameter whose literal value changed), or has no run in the conversation at all (a new or condensed chat). An empty part is stated as having no script yet.

A completed turn's closing message is shown as the AI wrote it; nothing is appended to it. The `run_script` tool result the AI reads carries per-part measurements ("Parts: name: …; name: …") under each part's name when the script completed more than one part.

Source: `crates/cadmark-ui/src/chat.rs` (`show_step_run`, `tool_group_label`, `tool_call_label`), `crates/cadmark-app/src/app.rs` (`finish_turn`, `record_tool_start`), `crates/cadmark-bridge/src/openai_compatible.rs` (`ResponseAssembly`), `crates/cadmark-app/src/turn.rs` (`current_script_block`, `last_successful_run`, `describe_model`), `crates/cadmark-core/src/geometry.rs` (`describe_parts`).

The **Skills** menu inserts a built-in command into the draft. Start a message or
spatial comment with `/3d-printing` or `$3d-printing` to apply printing guidance
to that turn; active skills are shown beside the menu. See the
[printing skill guide](../guides/3d-printing.md).

## Spatial comments

Click geometry to open the comment overlay near the selection point. Type free text and press Enter to submit; Escape cancels. Clicking more geometry while the overlay is open adds anchors — one comment can reference several elements.

Comments stay as editable, removable pending cards and send together with optional chat text as one turn. Their viewport highlights share each card's colour.

For ambiguous anchors (geometry traceable to multiple source lines), the overlay lists candidate lines — ranked by likelihood where the ledger can, and in recorded order where it cannot. Hovering a candidate highlights its line in the code panel and the geometry that line accounts for in the viewport. Choosing a candidate sends that line alone to the AI; choosing none sends all candidates.

Source: `crates/cadmark-ui/src/overlay.rs`, `crates/cadmark-ui/src/chat.rs:60–73`.

## Left panel: parameters and parts

Two tabs behind one strip (`crates/cadmark-ui/src/side_panel.rs`), chosen by `CadmarkApp::side_panel_tab`; the strip also shows the script's file name and the part count.

### Parameters

Lists every module-level numeric name in the open part's script. Names bound to a literal show a drag/type field; names derived from other parameters show their expression and are read-only.

Editing a value rewrites that one number in the script, rebuilds the model, and records a design step. No AI turn is involved.

Each row ends in a padlock. A **locked parameter** carries a `# locked` comment at the end of its binding's line in the script (`# locked: reason` optionally; the marker is the last `#` segment in the statement's own tail — the bytes after its last token up to its line ending, a `\r` excluded — whose text begins `locked`, case-insensitive, so an author's comment before it survives; a statement a semicolon follows has no tail, so the line's comment belongs to the statement that ends the line and `lock` refuses the earlier one with `RewriteError::SharedLine`). Clicking the padlock writes or removes the marker (`script_parameters::lock`, `unlock`), records a design step ("Locked width" / "Unlocked width"), refreshes the panel from the rewritten text, and adds a design-change note to the conversation; nothing rebuilds when the file on disk is the executed script, since a marker changes no geometry, while a file that already differs on disk — an edit made outside CADmark — is written with the marker and reloaded (a click that changes nothing in such a file reloads it too, so the panel catches up). The user's own value edits are not blocked by a lock. Derived parameters lock too: the expression is the constraint. A rebinding decides the lock as it decides the value.

The lock binds the AI. The system prompt tells it never to change, rename, unlock, or derive around a locked parameter, to end the turn and ask when a request needs one to move, to change it only on the user's explicit permission in the message, and to lock hard requirements the user states. Mechanically, the turn loop compares the script at turn start with each successful run: a locked parameter that changed, was unlocked, or was removed is named in that run's tool result with the rule restated (`locked_parameter_changes` in `crates/cadmark-app/src/turn.rs`), and a completed turn returns any such change on `TurnOutcome::Completed { locked_changes }`, which `finish_turn` posts as a notice after the AI's closing message (not as a turn event, which would open a second AI reply). A module-level augmented assignment the script did not have before (`wall += 1`, `script_parameters::augmentations`) counts as a change to a locked parameter even though its binding stands. A parameter first locked during the turn is not a change. A lock on a derived parameter fixes its expression: a change to an unlocked input that moves the number it produces is not a change to it, and the check does not evaluate expressions; the system prompt tells the AI to lock the inputs too when the produced number must hold, and to state a hard requirement as a literal. `lock` and `unlock` refuse a parameter bound in one statement with another (`width, depth = 80, 40`) with `RewriteError::SharedStatement`, since one marker would lock both; both rows still read as locked when the marker is there.

Source: `crates/cadmark-ui/src/parameters.rs`, `crates/cadmark-app/src/script_parameters.rs` (`Lock`, `trailing_lock`, `lock`, `unlock`), `crates/cadmark-app/src/app.rs` (`apply_parameter_lock`), `crates/cadmark-app/src/turn.rs` (`locked_parameter_changes`), `crates/cadmark-bridge/src/system_prompt.md`.

### Parts

One row per `LoadedPart` in binding order: a swatch in the part's palette colour, the name, a visibility tick box, and a warning glyph when the part is not export-ready. The active part's row is raised. Clicking a name selects the whole part (`PartsAction::Select`); the tick box hides or shows it (`PartsAction::SetVisible`).

A part's name is the shape's `label` when the script set one (`lid.label = "lid"`, or `bp.part.label = "lid"` after a `BuildPart`), otherwise the binding the shape was found under; an alias does not rename a labelled part. Names are unique within one execution: a later part repeating an earlier name is numbered in script order (`lid`, `lid (2)`). The name is the part's identity across executions, used by the hidden set, the run result's per-part measurements, and `render_view`'s `part` argument (`part_name`, `disambiguate_names` in `crates/cadmark-kernel/src/tessellation.rs`).

A hidden part's `GpuMesh::visible` is false, so every renderer pass skips it: it is not shaded, not in the picking texture, and not in the depth prepass. The set of hidden parts is kept on the project by part name (`Project::hidden_parts`) and reapplied to each freshly executed model by name, since part ordinals are not stable across executions; it is emptied when another script is opened (`Project::switch_part`), and a new project starts with none. Hiding the active part clears the selection. The same set is published to the AI's render source (`SceneHandle::set_hidden`), so a `render_view` without `part` draws exactly the parts the viewport shows (`parts_to_draw` in `crates/cadmark-app/src/render_source.rs`).

Source: `crates/cadmark-ui/src/parts.rs`, `crates/cadmark-app/src/app.rs` (`show_side_panel`, `set_part_visible`, `hidden_part_ids`), `crates/cadmark-app/src/render_source.rs`.

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

### Read-back proof

Every solid export is proven by reading the written file back and comparing it with the retained model. The comparison is format-specific:

- **STEP**: solids, faces, volume and size.
- **STL and 3MF**: closed shells (not solids — an STL of two bodies reads back as one solid of two shells), volume and size.

Two tolerances govern the comparison (`crates/cadmark-core/src/export.rs`):

| Tolerance | Value | Applies to |
|-----------|-------|------------|
| `EXACT_TOLERANCE` | 0.01 % | STEP without conversion |
| `APPROXIMATE_TOLERANCE` | 1 % | STL, 3MF, and STEP after B-spline conversion |

A relative comparison below `GEOMETRIC_CONFUSION` (1e-7 mm) uses the absolute floor, so a zero-valued expectation (a sketch's volume, a flat profile's thin axis) tolerates floating-point noise without a sub-millimetre feature escaping its percentage.

Drawing formats (SVG, DXF) carry a profile, not a part, and are written without a reproduction check.

### Refusal

A file that does not reproduce the part within its tolerance is removed, and the export is refused. The status bar shows the refusal with its cause from `ExportReport::explain` (`crates/cadmark-core/src/export.rs`).

For STEP, the cause names the faces the format lost with their surface and curve kinds (e.g. "5 faces on extrusion surfaces bounded by line, and offset curves"), built from `lost_faces` via `describe_lost_faces`. For STL and 3MF, the cause names the discrepancies: shell count, volume and size differences. Both formats report the discrepancies built by `ExportReport::discrepancies`.

A refused export is also recorded in the conversation as a `Message::export_refusal` (`MessageKind::ExportRefusal` in `crates/cadmark-core/src/message.rs`). This message appears in the chat as a muted CADmark note and is replayed to the AI's next turn as a `"Note from CADmark: …"` user item (`crates/cadmark-app/src/turn.rs`), so the AI reads the refusal and its cause. A refusal that arrives while a turn is already running reaches the next turn that assembles a request after it (`crates/cadmark-app/src/app.rs`, `Exported` arm; the conversation is saved immediately on refusal).

### STEP offset-curve conversion

OCCT's STEP writer omits faces built on offset curves (`Geom_OffsetCurve`) and reports success. When a plain STEP write loses geometry, the kernel converts those faces to B-splines with `ShapeCustom::BSplineRestriction` and writes the file again (`_cadmark_convert` in `crates/cadmark-kernel/src/export.rs`). The conversion only rewrites offset curves and the extrusion and offset surfaces built on them; planes, conics and ordinary B-splines pass through untouched.

The export message states the volume deviation the conversion introduced (a fraction of a percent), and the file is judged at `APPROXIMATE_TOLERANCE` (1 %) rather than `EXACT_TOLERANCE` (`ExportReport::tolerance`). If the converted file still does not reproduce the part, it is removed and the refusal says "converting them to B-splines did not recover the file either."

Offsets of lines and arcs simplify to lines and arcs and need no conversion. STL and 3MF carry offset geometry as triangles and need no conversion either; a mesh gets no second attempt (`prove` in `crates/cadmark-kernel/src/export.rs`).

### Validity gate versus reproduction gate

The validity gate (`describe_validity` in `crates/cadmark-app/src/validity.rs`) reports whether the solid is closed and valid after each build. The reproduction gate (this export proof) reports whether the written file reads back as the part. A part can be valid but not reproducible in a given format (faces the writer cannot carry), or reproducible but flagged as invalid (an open shell that meshes faithfully).

Source: `crates/cadmark-core/src/export.rs`, `crates/cadmark-kernel/src/export.rs`, `crates/cadmark-app/src/app.rs` (`Exported` arm), `crates/cadmark-core/src/message.rs`, `crates/cadmark-app/src/turn.rs`.

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
| `Alt`+click | Select the whole part under the cursor |

All shortcuts are inactive while a text field has focus, except `Ctrl+E` and `Ctrl+,`. Undo, redo, open, save, and rebuild are also inactive while the AI is working or a dialog is open.

Source: `crates/cadmark-app/src/app.rs:1353–1403`.
