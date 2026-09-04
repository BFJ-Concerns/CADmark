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

Window title carries the project folder name. The project menu shows the folder path, offers "Open project folder" (`Ctrl+O`), "New conversation", "Open recent", opening the script in an external editor, and showing the folder in the file manager.

Source: `crates/cadmark-ui/src/toolbar.rs:139–322`.

## Viewport

3D viewport rendered with wgpu. Navigation:

| Control | Action |
|---------|--------|
| Right-drag | Orbit |
| Middle-drag | Pan |
| Scroll | Zoom |
| Click | Select geometry / place comment |

The world is Z-up (build123d convention). The status bar shows navigation controls as a hint.

Source: `crates/cadmark-renderer/src/camera.rs:1–11`, `crates/cadmark-ui/src/status.rs:96–99`.

## Selection

Clicking geometry selects it: faces, edges, vertices. Edges are drawn at a clickable screen-space width and every vertex gets a marker; both hold their size on screen at any zoom. "Pick part" in the toolbar selects a whole part with the next click.

Sketch elements are selectable when the design has reached only a sketch: curves, corners, and regions draw face-on to the sketch's plane in orthographic view, with any existing solid ghosted behind them.

Selected elements glow in the viewport. The status bar shows which element is selected. A vertex marker on geometry the section plane has cut away is neither drawn nor pickable.

### Select menu

The "Select" menu in the toolbar controls which element kinds a viewport click can land on. Three checkboxes — "Faces", "Edges", "Vertices" — all on by default. Disabling a kind makes the click pass through it to whatever is behind. The button label reads "Select" when all kinds are on, or "Select: edges, vertices" (for example) when one is off.

The filter does not affect whole-part picks or sketch element picks.

Source: `crates/cadmark-ui/src/toolbar.rs:551–570` (menu UI), `crates/cadmark-renderer/src/picking.rs:38–63` (`SelectionFilter`), `crates/cadmark-renderer/src/shaders/vertex_markers.wgsl:105–109` (section discard).

## Standard views and projection

Seven standard views: Front, Back, Left, Right, Top, Bottom, Isometric. Available from the "View" menu in the toolbar.

Two projection modes: Perspective (default) and Orthographic. Toggled via the View menu ("Switch to Perspective/Orthographic") or the `P` key.

Source: `crates/cadmark-renderer/src/camera.rs:22–37`, `crates/cadmark-ui/src/toolbar.rs:468–489`.

## Section plane

Cuts the model along X, Y, or Z. Toggled with the "Section" button in the toolbar. When active, the toolbar shows:

- Axis buttons (X, Y, Z) to choose the cut direction
- A slider to position the plane along the chosen axis, bounded by the model's extent
- A flip button to keep the other half

Cut-away geometry is also unclickable.

Source: `crates/cadmark-ui/src/toolbar.rs:51–83`, `crates/cadmark-ui/src/toolbar.rs:491–549`.

## Ghost mode

The "Ghost" button makes the model see-through. Another way to inspect internal geometry without cutting.

Source: `crates/cadmark-ui/src/toolbar.rs:542–549`.

## Chat pane

Conversation with the AI. Message types, distinguished by position and colour:

- User messages (right-leaning card)
- Spatial comments (accent-tinted, with chips naming each anchor's element and source line)
- AI replies (plain card, streamed as the turn progresses)
- Tool-call groups (collapsed by default, expandable to each call's input and result)
- Notices from CADmark (quiet, or red on failure)

While a turn runs: a phase line showing the current step, elapsed time, time since last event, and a Cancel button.

Input: multi-line text field. Enter sends, Shift+Enter breaks the line.

Context occupancy shown in the chat, including reserved reference-image budget, against the configured context-window setting.

Source: `crates/cadmark-ui/src/chat.rs:1–9`.

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
- "New conversation" archives the chat and starts a blank one; the script is unchanged

Named versions are highlighted in the history menu. Undo and redo tooltips describe what they will restore.

Source: `crates/cadmark-ui/src/toolbar.rs:328–428`.

## Export

Export menu in the toolbar. Available when a model is loaded. Formats:

| Format | Extension | Description |
|--------|-----------|-------------|
| STEP | `.step` | STEP AP214 B-rep for other CAD tools |
| STL | `.stl` | Binary STL mesh for slicers |
| 3MF | `.3mf` | 3MF mesh with units for slicers |

Export writes the file next to the part script. Multi-part models offer per-part and "Export all" options. An open or invalid solid shows a non-blocking warning before export.

Source: `crates/cadmark-core/src/export.rs`, `crates/cadmark-ui/src/toolbar.rs:562–610`.

## Solid validity

After each build, the status bar reports every produced part's validity. The message comes from `describe_validity`:

- "Part 1 is closed and valid." — a printable solid
- "Part 1 is NOT a closed valid solid; it will not print." — open or invalid geometry

When exporting, an invalid part shows a non-blocking warning: "Cannot export: Part N is not a closed valid solid."

Source: `crates/cadmark-app/src/validity.rs:33–53`, `crates/cadmark-app/src/app.rs:886–896`.

## Reference images

PNG and JPEG files (extensions `.png`, `.jpg`, `.jpeg`) placed in a `references/` directory inside the project folder are sent to the AI with every turn when `accepts_images` is enabled. Other file types are ignored. Their token budget is included in the context-occupancy display.

Source: `crates/cadmark-app/src/turn.rs:646–689`.

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
