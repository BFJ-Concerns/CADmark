---
description: Use the built-in printing skill to review designs, plan orientation, and check fits
---

# Design for 3D printing

Start a chat message with `/3d-printing` or `$3d-printing`, or choose
**3D printing** from the chat pane's **Skills** menu. The menu inserts the
command into your draft. Send it on its own to ask for a review of the current
design, or follow it with a request:

```text
/3d-printing Review this bracket for printing without supports.
$3d-printing Make a box with a sliding lid for filament printing.
```

A command must be the first word, followed by whitespace or the end of the
message. It also works at the start of a spatial comment. The chat pane
shows which skill will apply to the turn, including staged comments.
Commands mentioned later in prose or inside code fences are ordinary text.

The skill is built in and applies to the current turn. There are no custom
skill files or settings. Invoke it again when you want another printing
review. The current script and geometry attached to spatial comments give the model
context. A review asks for advice, while a request to change the design allows
edits.

## Give the model the constraints that matter

Mention the printing process and material, the nozzle or extrusion width,
and the layer height if known. Explain what the part mates with and where
loads act. A printer-independent limit cannot establish the fit or strength
of a particular print. If you do not know a setting, the model can state an
assumption and describe what to check.

The skill uses [Hydra Research's FFF design rules](https://www.hydraresearch3d.com/design-rules)
and [Prusa's modelling guidance](https://help.prusa3d.com/article/modeling-with-3d-printing-in-mind_164135).
Its [reference guide](../../agent-docs/guides/3d-printing.md) records the
starting values, their sources, and the clearance conventions. These rules
concern filament printing; resin and powder processes need different limits.

## Review the print as well as the CAD solid

Use the advice to decide an orientation and identify which features need a
small test print. Check that supports can be removed and that mating parts
can be assembled. Inspect thin walls and small details in the slicer preview.
A closed, valid CAD solid is only a geometry check; it does not prove that the
part will print well or carry its intended load.

For the modelling details behind a proposed change, see
[practical modelling recipes](modelling.md), including threads, clearance,
holes, enclosures, and edge finishing. The AI can retrieve these guides with
its documentation lookup tool during an ordinary modelling turn too.
