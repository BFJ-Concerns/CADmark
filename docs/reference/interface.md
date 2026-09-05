---
description: Viewport controls, toolbar, chat, panels, measurement, and keyboard shortcuts
---

# Interface

## Start view

When you launch CADmark without naming a project folder, it opens the start view: a list of folders you worked in recently, a button to open another, and one to create a new project in an empty folder. Nothing loads until you choose.

You can type into the chat before choosing a project — the message becomes the first turn of whichever project you then open.

## Projects and parts

A CADmark project is just a folder. Inside it, each part is a build123d Python script.

The toolbar shows the open project and part. The part menu lists every part in the folder and offers "New part". A new part is called "Untitled" until you save it for the first time (`Ctrl+S`), which asks for a name — that name becomes the file name.

From the project menu you can open another folder (`Ctrl+O`), switch to a recent project, open the script in your system editor, or show the folder in a file manager.

## The viewport

The 3D viewport uses wgpu for rendering. Navigation follows CAD conventions:

- **Right-drag** to orbit
- **Middle-drag** to pan
- **Scroll** to zoom
- **Click** on geometry to select it or place a comment

The world is Z-up, matching build123d's coordinate system.

## Selecting geometry

Click a face, edge, or vertex to select it — the element glows, and its identity appears in the status bar. Edges and vertex markers have smooth, fine outlines. Their invisible click targets are wider, so you can aim near them. Both sizes stay constant on screen however far you zoom. A vertex marker on geometry the section plane has cut away is neither drawn nor pickable. To select a whole part in a multi-part model, click "Pick part" in the toolbar first.

The **Select** menu in the toolbar turns each kind of click target on or off. Three checkboxes — "Faces", "Edges", "Vertices" — start all on. Disabling a kind makes clicks pass through it to whatever is behind. The filter does not affect whole-part picks.

When a design contains only a sketch, the view switches to face its plane in orthographic projection, and any existing solid is ghosted behind it. Sketch curves, corners and filled regions are displayed, but cannot yet be selected in the viewport.

## Standard views and projection

The View menu offers seven standard orientations: Front, Back, Left, Right, Top, Bottom, and Isometric. Two projection modes — Perspective (default) and Orthographic — can be toggled from the same menu or with `P`.

## Seeing inside the model

Two tools let you inspect internal geometry without exporting:

**Section plane** — the "Section" button in the toolbar cuts the model along X, Y, or Z. A slider positions the plane within the model's extent, and a flip button swaps which half is kept. Cut-away geometry is unclickable.

**Ghost mode** — the "Ghost" button makes the model see-through.

## Chat

The chat pane is where you talk to the AI. Messages are distinguished by position and colour rather than labels:

- Your messages lean right.
- Spatial comments carry accent-tinted chips naming each anchor's element and source line.
- The AI's replies stream in as the turn progresses.
- Tool calls appear as a collapsed group you can expand to see each call's input and result.
- Notices from CADmark itself are quiet, or red when something failed.

While a turn is running, the pane shows which step the AI is on, how long it has been at it, and a Cancel button. The context-occupancy bar shows how much of the configured context window is in use, including any reference images.

Type with Enter to send, Shift+Enter for a line break.

"New conversation" at the top of the chat pane archives the current chat and starts blank. The script stays as it is. This button is unavailable while a build or AI turn runs.

The **Skills** menu inserts a built-in command into the draft. Start a message or
spatial comment with `/3d-printing` or `$3d-printing` to apply printing guidance
to that turn; active skills are shown beside the menu. See the
[printing skill guide](../guides/3d-printing.md).

## Spatial comments

Click on geometry to open a comment overlay anchored to the selection. Type what you want changed and press Enter (Escape to cancel). While the overlay is open, clicking more geometry adds anchors — one comment can point at several elements.

Comments stay as pending cards you can edit or remove before sending. They are sent together with any chat text as a single turn, and their viewport highlights share each card's accent colour.

When a selection could have come from more than one source line, the overlay lists the candidates — ranked by likelihood when possible, and plainly unordered when it cannot tell. Hovering a candidate highlights its line in the code panel and the geometry that line accounts for. Choosing one sends that line alone to the AI; leaving the choice open sends them all.

## Parameters panel

The panel beside the viewport lists every module-level numeric name in the current script. Names bound to a literal have a drag-or-type field; names derived from other parameters show their expression.

Changing a value rewrites that one number in the script, rebuilds the model, and records a design step — no AI turn needed.

## Code panel

Toggle with the "Code" button or `Ctrl+E`. A read-only view of the build123d script with line numbers, the selected element's source line highlighted, and a warning when the file has changed on disk. Buttons to copy the source, open it in your editor, or rebuild from disk (`F5`).

## Status bar

The bottom bar reports, from left to right:

- What is happening (build in progress, AI turn phase) or the last outcome
- The model's measurements — face count, volume, bounding box
- The current selection
- A measurement for the selected element (see below)
- Navigation controls as a reminder

## Measurement

Selecting an element shows its measurement in the status bar automatically:

- A **face** shows its area in mm².
- A **straight edge** shows its length in mm.
- A **circular edge or arc** shows its diameter in mm.
- Select **two elements** to see the minimum distance between them.

## Design history

Every accepted AI edit is saved as a design step. Steps record which part they changed, so undoing opens that part.

- `Ctrl+Z` / `Ctrl+Shift+Z` — undo and redo. The tooltip says what will be restored.
- The history menu lists every step (newest first, current one marked). Named versions are highlighted. Click a step to jump to it.
- `Ctrl+S` records the current state as a named version you can return to.

## Export

The Export menu in the toolbar writes the model to a file next to the part script. Three formats:

| Format | Use |
|--------|-----|
| STEP (.step) | Interchange with other CAD tools (AP214 B-rep) |
| STL (.stl) | Slicer input (binary mesh) |
| 3MF (.3mf) | Slicer input (mesh with units) |

Multi-part models can export one part or all parts. An open or invalid solid shows a warning before export.

## Solid validity

After each build, the status bar reports every produced part's validity:

- "Part 1 is closed and valid." — a solid that will print and export correctly.
- "Part 1 is NOT a closed valid solid; it will not print." — open or invalid geometry that needs fixing.

The export menu shows a warning before writing an invalid part.

## Reference images

Place PNG or JPEG files (`.png`, `.jpg`, `.jpeg`) in a `references/` folder inside the project directory. When the provider's "accepts images" setting is on, these images are sent with every AI turn so the model knows what you are aiming for. Other file types are ignored.

## Keyboard shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z` | Redo |
| `Ctrl+O` | Open project folder |
| `Ctrl+S` | Name this version |
| `Ctrl+E` | Toggle code panel |
| `Ctrl+,` | Open settings |
| `F5` | Rebuild |
| `F` | Fit view |
| `P` | Toggle projection |

Shortcuts that change model state (undo, redo, rebuild, save) are held while the AI is working or a dialog is open.
