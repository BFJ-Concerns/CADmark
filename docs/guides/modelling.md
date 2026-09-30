---
description: How to approach bolts, helix sweeps, mating parts, holes, enclosures, and edge finishing
---

# Common modelling tasks

These patterns help when a shape is simple to describe but awkward to build.
Ask CADmark to look up the named topic when it needs a recipe. Its bundled
reference includes the [complete examples and API details](../../agent-docs/guides/modelling.md).

## Make a threaded bolt with a helix

A helix describes the path of the thread. It has no solid thread profile on
its own. A bolt needs a cylindrical shaft and a closed profile swept around
that path, joined into one solid. State the pitch, thread length, major
diameter, and whether it must fit purchased hardware.

For example, ask: “Look up the threaded bolt helix sweep recipe and make a
coarse demonstration bolt.” The reference's runnable example places the
profile in the radial/axial plane, preserves its position with `align=None`,
and uses `is_frenet=True` for the sweep. Its root overlaps the shaft so the
union has material to join, and the ends are trimmed to the shaft length.
Keeping the profile narrower than the pitch prevents adjacent turns meeting.

That example is a demonstration thread. A standard nut needs the appropriate
standard thread geometry and fit, and a printed mating pair needs a calibrated
allowance. Start with a short bolt-and-nut test before a long threaded part.
Scaling a nut to loosen the fit changes its pitch as well as its diameter.

## Make two parts fit

Describe clearance per side or across the full diameter. A radial clearance
is a gap on each side of a round peg, so the hole's diameter grows by twice
that amount. The same distinction applies to a tongue centred in a slot.
Keep the hardware's nominal dimensions separate from printer compensation.

For a hexagonal nut pocket, specify the distance across the flats. The
reference explains `RegularPolygon`'s radius convention so the pocket does
not accidentally use a centre-to-corner dimension. Print a short test piece
before making the full housing.

## Put holes where the hardware needs them

Give the hole's axis, whether it passes through the part, and any head recess.
A counterbore has a cylindrical recess; a countersink has a conical recess.
For a blind hole, state the depth. The modelling recipe uses explicit planes
and cutter extents so a dimension change does not leave a thin membrane.

For filament printing, a horizontal circular hole has a roof to support.
Ask the printing skill to compare reorientation, a teardrop or chamfered
roof, and removable supports while preserving room for the mating shaft.

## Hollow a box and leave a floor

Specify wall thickness and floor thickness separately. For a rectangular
enclosure, subtracting an inner box placed above the floor makes both easy
to control; the reference includes a complete example. A lid needs its own
fit allowance rather than a scale applied to the whole lid.

An offset shell can suit a curved body, but tight radii and narrow passages
may prevent the requested thickness. Inspect the result with CADmark's
section plane to check the interior.

## Round the intended edges

Choose a face or feature first, then its edges. “Round the top rim” is more
stable than an edge number that may change after a boolean operation.
Finish the main shape before adding fillets and chamfers.

If a fillet fails, try a smaller radius and inspect nearby short edges and
thin walls. Test the selected edges individually to find the limiting area.
The [printing guide](3d-printing.md) helps choose a finish for bed-facing
edges. A valid result still needs a visual check that the intended edges
changed.

## Exports that keep every face

Every export is proven: the file is read back and compared with the model. A file that does not reproduce the part is refused and removed, and the message names what was lost — for STEP, the faces and their surface kinds; for STL and 3MF, the discrepancy in shell count, volume or size.

A part built on an offset of an ellipse or spline — using `offset` in a sketch, or `offset_2d` on a wire, then extruding or revolving it — gives faces that the STEP writer cannot carry as they are. CADmark converts those faces to B-splines before writing. The export message states the volume the conversion moved, typically around 0.1 % on a real part. The file is judged at a wider tolerance to accommodate the approximation.

Offsets of lines and arcs simplify to lines and arcs exactly and need no conversion. STL and 3MF carry offset geometry as triangles and need none either.

If the AI is working on a part with curved or offset geometry, ask it to run `check_export` before calling the part finished — it runs the same proof without keeping a file.
