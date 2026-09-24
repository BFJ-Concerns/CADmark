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

Click a face, edge, or vertex to select it — the element glows, and its identity appears in the status bar. Edges and vertex markers have smooth, fine outlines. Their invisible click targets are wider, so you can aim near them. Both sizes stay constant on screen however far you zoom. A vertex marker on geometry the section plane has cut away is neither drawn nor pickable. When the script defines several parts, each is drawn in its own colour, and clicking any of them selects that part's face, edge, or vertex; the status bar and the comment overlay then speak in that part's terms. To select a whole part, hold `Alt` and click it, or click its name in the Parts tab of the left panel.

The **Select** menu in the toolbar turns each kind of click target on or off. Three checkboxes — "Faces", "Edges", "Vertices" — start all on. Disabling a kind makes clicks pass through it to whatever is behind. The filter does not affect whole-part picks.

When a design contains only a sketch, the view switches to face its plane in orthographic projection, and any existing solid is ghosted behind it. Its curves, corners and filled regions can be clicked and commented on like a solid's edges, vertices and faces: a corner wins over the curves meeting at it, a curve over the region it bounds, and the Select menu's three checkboxes govern the three kinds. The ghosted solid is not a click target, since it belongs to an earlier version of the script.

A selected sketch element is measured like a solid's: a curve shows its length, a circular curve its diameter, and a region its area, and the status line gives the sketch's overall width and height in its own plane. The comment overlay names the line that drew the element — "drawn by Rectangle at line 4" — and where an operation such as `fillet` or `make_face` rebuilt the outline so that no drawn curve survives, it says which operation broke the route rather than guessing a line. On a finished solid, selecting a face or edge that came from a sketch lists the drawing line beneath its own origin; resting the pointer on that row brings the line into view in the code panel.

## View cube

The cube in the viewport's top-right corner turns with the model and is the way to reorient the view. Its six faces are labelled Front, Back, Left, Right, Top and Bottom. Click the middle of a face to look squarely at it, the strip along an edge to look from halfway between two faces, or a corner to look from an isometric direction — the region under the pointer lights up, and a tooltip names the view. Drag the cube to orbit freely. The curved arrows that appear beside it turn the view a quarter turn either way about the line of sight; snapping to any named view undoes the turn.

Beneath the cube: **Fit** frames the whole model (`F`), the projection button switches between perspective and orthographic (`P`) and reads which one is current, and **XYZ** shows or hides a small axis triad indicating which way X, Y and Z run. The triad is off by default, and the choice is remembered across sessions.

The camera zooms from a fraction of a millimetre to kilometres away, and the model stays drawn however far in or out you go.

## Seeing inside the model

Two tools let you inspect internal geometry without exporting:

**Section plane** — the "Section" button in the toolbar cuts the model along X, Y, or Z. A slider positions the plane within the model's extent, and a flip button swaps which half is kept. Cut-away geometry is unclickable, and a click there reaches the interior face the cut reveals.

**Ghost mode** — the "Ghost" button makes the model see-through.

## Chat

The chat pane is where you talk to the AI. Messages are distinguished by position and colour rather than labels:

- Your messages lean right.
- Spatial comments carry accent-tinted chips naming each anchor's element and source line.
- The AI's replies stream in as the turn progresses, and it says what it is about to do before each tool call or run of calls, so the text between the tool lines reads as a running commentary.
- Tool calls appear one line each while the turn runs, so you can watch them arrive; each line is collapsed and expands to the call's input and result.
- The AI's thinking shows too: "thinking…" while it reasons before a reply, then "thought for 2m 05s" once it goes on to speak or call a tool. Where the provider shares the reasoning text, the line opens to it; where it keeps the reasoning private, the line stands alone, and the phase line under the messages still counts the reasoning as activity rather than showing the stream as quiet.
- When the turn ends, each run of calls and thinking folds into a single collapsed line such as "8 tool calls: ran the script ×6, looked up docs ×2 · thought for 4m 10s", which opens to the same lines. What the AI wrote between the steps stays shown.
- Notices from CADmark itself are quiet, or red when something failed.

If a turn fails or you cancel it, whatever the AI had written so far stays in the chat, above the notice that says how the turn ended. If the provider cuts a reply off at its output limit, CADmark keeps what was written, runs the tool calls the AI had finished writing, says in the chat that the reply was cut off, and asks the AI to carry on. CADmark can also continue when the provider returns a complete, resumable reasoning item without visible text. If there is no text, complete tool call or resumable reasoning, the turn ends with a message naming the output limit. When a provider reports a usage limit or overload partway through a reply, the message names that cause.

Every turn sends the AI the part's script as it stands on disk, whatever the conversation holds: a reopened project, a new conversation, or a condensed chat all start from the real file. When the file differs from the last script the AI ran, because a value was changed in the parameters panel or a design step was undone, the AI is told so and which parameter values changed, so it keeps them. Each such change is also noted in the chat where it happened, in a quiet card the AI reads with the rest of the conversation, so it knows when you set a value or stepped back and can tell one edit from a later reversal.

Within a turn the AI works on the script the way a coding assistant works on a file. It can replace the whole script for a new part or a rewrite, or change just the lines it means to by giving the exact text to replace and its replacement, then run the file as edited. It can read the script with line numbers to follow a traceback, and it can run scratch Python after the script, in the script's own namespace, to measure a face, list edges, or check a clearance before deciding what to change; nothing a scratch run does reaches the script or the model. Anything the script prints comes back to the AI with the run's result, and so does what it printed before a failure. Each of these appears in the chat's tool-call group as it happens. Only a successful run counts: an edit the AI never ran successfully is discarded when the turn ends, and the script is left as the turn found it.

A completed turn's reply ends with what measurably changed: face count, volume, and overall size before and after, so an edit that did more than you asked is visible at once. When the script defines several parts, each part is reported on its own line under the name the script gives it, with a part the script no longer produces marked as removed and one it did not produce before as new.

While a turn is running, the pane shows which step the AI is on, how long it has been at it, and a Cancel button. The context figure above the messages shows how much of the configured context window the next request would occupy: the conversation, the reference images, and everything else a request carries (the AI's instructions, its tools, the current script, the examples chosen for the request, and what you have typed). Hover it for the breakdown. It turns amber with a warning when the script, instructions and images alone nearly fill the window, because condensing the conversation cannot help then; raise the context window in Settings or shorten the script.

That figure is CADmark's own estimate. Under it, once a turn has had a reply, is what the provider actually counted for the last request: the tokens it read, how many of those came from its cache, the tokens the model wrote, and how many of those were spent reasoning. These are the provider's reported figures; missing cache or reasoning details read "not reported". An empty visible reply does not by itself show what consumed the output limit: reasoning and an unfinished tool call are both possible. The request record holds the events and usage needed to investigate.

CADmark keeps the model's conversation in order across turns and project reopening, including its renders and encrypted reasoning state. New context is added after the saved history, so earlier work remains available to the model and eligible for prompt-cache reuse. Cache hits still depend on the provider's settings and retention. The thinking line in chat is the display of that activity; it is not sent back as another reply.

Starting a new conversation or condensing a long one begins a fresh sequence. Changing the endpoint, model or image capability rebuilds the model's history from the chat, keeping opaque reasoning from one backend out of another. If you interrupt a tool call, its saved result tells the model that the call did not return a result.

Every request the AI is sent, and everything it streams back, is written to the project folder under `.cadmark/requests/`, one file per call, as it happens. When a turn seems stuck, the newest file there shows what was sent and what has arrived so far; when it ends, the last line says how and what it cost. The newest sixty files are kept. Your credential is never written there.

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

## Left panel: parameters and parts

The panel beside the viewport has two tabs, Parameters and Parts, with the script's file name at the right of the strip. The Parts tab shows how many parts the script defines.

### Parameters

Lists every module-level numeric name in the current script. Names bound to a literal have a drag-or-type field; names derived from other parameters show their expression.

Changing a value rewrites that one number in the script, rebuilds the model, and records a design step — no AI turn needed.

### Parts

Lists every completed part the script produced, in the order the script binds them: a colour swatch matching the part in the viewport, the part's name, and a tick box for whether it is drawn. The part the current selection belongs to is highlighted. Hovering a name shows its measurements; clicking it selects the whole part. A part that is not a closed solid carries a warning mark.

Unticking a part hides it: it is neither drawn nor clickable, and nothing behind it is hidden by it any more. A hidden part stays hidden by name through rebuilds, so a part you have set aside stays out of the way while the AI works on the others; opening another script or project shows everything again. Hiding the part that holds the current selection puts the selection down.

Parts are coloured from a fixed palette by their position in the script: the first part keeps the grey a single-part model has always had, and later parts take distinct muted hues.

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

The Export menu in the toolbar writes the model to a file next to the part script. A solid offers three formats:

| Format | Use |
|--------|-----|
| STEP (.step) | Interchange with other CAD tools (AP214 B-rep) |
| STL (.stl) | Slicer input (binary mesh) |
| 3MF (.3mf) | Slicer input (mesh with units) |

A design that has reached only a sketch offers drawing formats instead, written flat in the plane the sketch was drawn on and at true size in millimetres:

| Format | Use |
|--------|-----|
| SVG (.svg) | Vector drawing for laser cutters, plotters and illustration tools |
| DXF (.dxf) | 2D CAD and CNC toolpath input |
| STEP (.step) | The sketch's faces and curves for another CAD tool |

Multi-part models can export one part or all parts; the top-level entries write the part currently selected and name it in the menu. An open or invalid solid shows a warning before export.

## Solid validity

After each build, the status bar reports every produced part's validity:

- "Part 1 is closed and valid." — a solid that will print and export correctly.
- "Part 1 is NOT a closed valid solid; it will not print." — open or invalid geometry that needs fixing.

The export menu shows a warning before writing an invalid part.

## Images in chat

Attach a photo or drawing to a message in any of three ways:

- Click **Attach…** below the input and choose one or more files. The picker shows every file; CADmark reads the format from the file's contents, so a download saved without a `.png` or `.jpg` extension still works.
- Paste with `Ctrl+V` while the input has focus. A screenshot or an image copied from a browser is attached as a PNG.
- Drop files from a file manager onto the CADmark window.

Attached images show as thumbnails above the input until you send. Click the × on a thumbnail to remove it. An image can be sent on its own, without any text. The message keeps its images in the conversation, and the AI sees them again whenever that message is part of the history, in this session and after the project is reopened. Copies are kept in the project's `.cadmark/attachments/` folder; the originals can be moved or deleted.

Use an image-capable AI and tick **The model reads images** in Settings. With a text-only AI the strip says so: the message is sent with the image names only.

### The reference library

Pictures that define a part, such as a dimensioned drawing or a photo of the original, are worth keeping beyond one conversation. The AI keeps them in the project's `references/` folder when they matter, with a line in `references/INDEX.md` describing what each shows and what it is for. The rest of that file is yours: notes you add to it are left alone when the AI updates a line. In a later conversation it reads that index, lists the library, and looks at any image it needs. Ask it to keep an image if it has not, or to describe one you have placed in the folder by hand.

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
| `Alt`+click | Select the whole part under the cursor |

Shortcuts that change model state (undo, redo, rebuild, save) are held while the AI is working or a dialog is open.
