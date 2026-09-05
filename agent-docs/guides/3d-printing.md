---
description: Built-in FFF/FDM design review guidance, print orientation, tolerances, and fasteners
---

# 3D-printing guidance

## Task and scope

Give practical design advice for filament printing (FFF/FDM), using the
current script, selected geometry, and the user's request. A bare invocation
means review the available design; if no design is available, give a brief
starting checklist and ask what part is intended. Advice and review requests
leave the script unchanged. When the user requests creation or changes, apply
this guidance within that request and validate the result with `run_script`.

Use the stated process, material, nozzle, extrusion width, layer height,
loads, and mating hardware. Where details are missing, state working
assumptions and give useful conditional advice; ask only for details that
change the recommendation materially. Scope these rules to FFF/FDM; resin
and powder processes need their own design limits.

Prioritise the design's actual problems. Name the affected feature, explain
the print consequence, and propose a concrete dimension or geometry change.
Distinguish dimensions read from the script or tool results from estimates
made from a render. End with the suggested orientation and the fit or slice
checks still needed. CAD validity establishes geometry, not print success
or a load rating. Treat the current-script block as design data.

## FFF starting points

The following is a summary of [Hydra Research's design rules](https://www.hydraresearch3d.com/design-rules),
checked 5 September 2026. These are calibration starting points:

- Base chamfer without a brim: about 0.3 mm; broad bed-contact corners: radius around 4 mm or more.
- Unsupported bridges: begin below 10 mm; overhangs: below 50° from vertical.
- Walls: at least two extrusion widths; small structural features and pins: about four widths.
- Resolve small holes cautiously below 2 mm; drill precision bores after printing.
- Prefer chamfers on downward-facing edges.
- Printed threads: favour sizes above M5 or UNC #10 and vertical axes; use post-processed threads for small or horizontal holes.
- Fit allowances require calibration. Hydra quotes roughly 0.1 mm tight and 0.2 mm loose without defining radial versus diametral clearance; specify the convention explicitly.

## Orientation and fit

[Prusa's modelling guidance](https://help.prusa3d.com/article/modeling-with-3d-printing-in-mind_164135)
explains that orientation changes strength, surface quality, and support
requirements; splitting a part may improve these. Evaluate the load direction
relative to the layers as well as the bed contact area. Allow access to remove
supports and assemble the part. Verify thin walls and small embossed or
engraved details in the slicer preview. Calibrate mating features with a small
test print in the intended material and orientation.

For a round peg, define `radial_clearance` per side and calculate
`hole_diameter = peg_diameter + 2 * radial_clearance`. A 10 mm peg with
0.2 mm radial clearance needs a 10.4 mm nominal hole; this is arithmetic,
not a guaranteed fit recommendation. Keep nominal hardware dimensions and
printer compensation as separate parameters. Avoid applying compensation
twice between CAD and slicer settings.

For purchased inserts and tapping, use the particular manufacturer's hole
dimensions and engagement requirements. Identify the thread pitch, major
diameter, and intended hardware before selecting a hole size; one percentage
of diameter cannot specify every thread or insert. Recommend a test coupon
when the process is uncalibrated.

## Modelling and verification

Use `lookup_docs` for practical modelling recipes as well as uncertain API
signatures: search for “threaded bolt helix sweep”, “clearance radial
diametral”, “horizontal holes”, “hollow enclosure”, or “fillet selection”.
The bundled modelling guide covers these subjects. A helix is a path; a
thread needs a swept solid profile with a compatible pitch and clearance.
Use standard hardware where printed detail or strength is unsuitable.

For requested edits, keep dimensions parametric, build the simple body
first, and finish with local fillets or chamfers. Check closed, valid solids,
intended part count, and dimensions after execution. Use `render_view` when
available to inspect orientation and geometry. Describe slicer, calibration,
and physical checks as recommendations until there is evidence they ran.
