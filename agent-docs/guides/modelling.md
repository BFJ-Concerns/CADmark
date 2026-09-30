---
description: build123d recipes for threaded bolts, helix sweeps, fits, holes, enclosures, and fillets
---

# Practical modelling recipes

## Threaded bolt: helix and sweep

Search terms: screw, bolt, nut, thread, pitch, Helix, sweep, is_frenet.

A `Helix` is a curve, not a thread solid. Sweep a closed face along it,
fuse the ridge with a cylindrical core, and trim the ends. This example
uses build123d 0.11.1 and makes one solid with a hexagonal head. It is an
illustrative coarse thread, not an ISO metric or UNC thread and not a
claim of compatibility with purchased nuts.

```python
from build123d import *

core_radius = 5.0
pitch = 2.0
thread_depth = 0.8
thread_half_width = 0.6
thread_length = 10.0
root_overlap = 0.15
head_radius = 8.0
head_height = 3.0

path = Helix(pitch=pitch, height=thread_length, radius=core_radius)
profile = Plane.XZ * Polygon(
    (core_radius - root_overlap, -thread_half_width),
    (core_radius + thread_depth, 0),
    (core_radius - root_overlap, thread_half_width),
    align=None,
)
with BuildPart() as bolt:
    Cylinder(core_radius, thread_length, align=(Align.CENTER, Align.CENTER, Align.MIN))
    sweep(sections=profile, path=path, is_frenet=True)
    # Trim the swept ends to the shaft length.
    Cylinder(core_radius + thread_depth + root_overlap, thread_length,
             align=(Align.CENTER, Align.CENTER, Align.MIN), mode=Mode.INTERSECT)
    with BuildSketch(Plane.XY.offset(-head_height)):
        RegularPolygon(radius=head_radius, side_count=6)
    extrude(amount=head_height)
```

For this Z-axis helix the start lies on +X. `Plane.XZ` holds the radial/axial
profile; `align=None` preserves its coordinates relative to the shaft axis.
`is_frenet=True` transports the section along the helix using its Frenet
frame. Keep `2 * thread_half_width < pitch` so neighbouring turns have a
gap; the root extends inside the shaft by `root_overlap` so the union has
volume overlap. Merely touching the core can leave separate or invalid
solids. The clipping cylinder removes the protruding start and end profiles.

Keep the helix and profile as curve/sketch objects; the only completed
`BuildPart` here is `bolt`. Intermediate top-level solids can appear as
additional parts in CADmark. When building in algebra mode, rebind one name
or keep intermediate solids inside a function.

Check validity, solid count, pitch, major diameter, and usable engagement
length. For a matching nut, derive the internal cutter from the same pitch,
handedness, and profile, with explicit fit allowances; enlarging the whole
nut also changes its pitch. Start with a short mating test piece. Real
hardware needs the specified standard's profile, truncations, pitch,
handedness, and fit class. If a sweep fails, test one turn, check the section
plane and overlap, then extend it. Fillet or chamfer selected simple edges
only after the thread boolean succeeds.

## Fits: radial versus diametral clearance

Search terms: tolerance, peg, socket, press fit, sliding fit, nut trap.

Name nominal size and clearance separately. For a round mating pair:
`hole_diameter = peg_diameter + 2 * radial_clearance`.
For a centred rectangular tongue: `slot_width = tongue_width + 2 * side_clearance`.
For a hexagonal nut pocket, `RegularPolygon(radius=across_flats / 2,
side_count=6, major_radius=False)` uses the apothem (centre to flat);
its default `major_radius=True` instead uses the centre-to-vertex radius.
Add the pocket allowance to `across_flats` before halving it.

Keep fit calibration separate from nominal hardware dimensions. Print a
short coupon with the same orientation and process before committing to a
long thread or full enclosure. The [3D-printing guide](3d-printing.md)
covers process assumptions and clearance conventions.

## Holes, counterbores, and horizontal holes

Search terms: through hole, blind hole, countersink, counterbore, teardrop.

Place cutters on an explicit workplane and use explicit depths for blind
features. A centred `Cylinder` extends on both sides of its location;
`align=(Align.CENTER, Align.CENTER, Align.MIN)` starts it at local Z=0.
For a through cut, extend the cutter past both faces to avoid residual
slivers when dimensions change. `CounterBoreHole` and `CounterSinkHole`
combine the bore and head recess; look up their exact radius and depth
arguments before using them.

A circular hole with a horizontal axis presents an unsupported roof in
filament printing. Consider reorientation, an apex-up teardrop profile,
a chamfered roof, or accessible supports. These change the available
clearance: preserve the required circular envelope for a mating shaft and
check the sliced roof. Choose the hole axis from its function first.

## Hollow enclosure with a controlled floor

Search terms: box, shell, offset, wall thickness, lid, cavity.

An outer box minus an explicitly positioned cavity makes wall and floor
dimensions independent. Both boxes below start at their local Z=0; the
cavity begins above the floor and extends beyond the top. Dimensions must
leave positive inner width, depth, and cavity height.

```python
from build123d import *

width = 40.0
depth = 30.0
height = 15.0
wall_thickness = 2.0
floor_thickness = 2.0
cut_extension = 1.0

with BuildPart() as enclosure:
    Box(width, depth, height, align=(Align.CENTER, Align.CENTER, Align.MIN))
    with Locations((0, 0, floor_thickness)):
        Box(width - 2 * wall_thickness, depth - 2 * wall_thickness,
            height - floor_thickness + cut_extension,
            align=(Align.CENTER, Align.CENTER, Align.MIN), mode=Mode.SUBTRACT)
```

For curved bodies, `offset` with an opening face can produce a shell, but
an offset may fail where thickness exceeds a local radius or closes a
narrow passage. Inspect a section after shelling. Model lid fit allowances
separately; do not scale the whole lid to create clearance.

## Fillets, chamfers, and stable selections

Search terms: round edge, edge selector, sort_by, filter_by, radius failure.

Select from a known feature or face instead of using a global edge index.
On a simple upright box, `part.faces().sort_by(Axis.Z)[-1]` selects its top
face; this assumption needs revisiting once other higher features exist.
Take that face's edges for a top rim treatment, or use
`part.edges().filter_by(Axis.Z)` for straight vertical edges.

Perform booleans and major dimensions before selecting finishing edges;
old topology references may no longer identify the intended feature.
When a fillet fails, reduce the radius, check neighbouring wall thickness
and short edges, and try the intended edges individually. A fillet that
works on the outside may consume a thin inside wall. Inspect a section and
check validity after each change. For bed-facing edges in FFF/FDM, consult
the [3D-printing guide](3d-printing.md) before choosing the finish.

## Exports that keep every face

Search terms: STEP, export refused, offset curve, faces missing, B-spline
conversion, check_export.

Every export is proven: the file is read back and compared with the model
— solids and faces for STEP, closed surfaces for STL and 3MF, volume and
size for both. A file that does not reproduce the part is refused and
removed, and the refusal names the faces the format lost with their surface
and curve kinds. `check_export` runs the same proof inside a turn without
keeping a file, and gives the same answer.

Faces built on an offset of a conic or spline cannot be written to STEP as
they are: an ellipse or spline grown with `offset` in a sketch, or a wire's
`offset_2d`, then extruded or revolved, gives faces over `Geom_OffsetCurve`
geometry, which OCCT's STEP writer omits while reporting success. CADmark
converts those faces to B-splines before writing, states the volume the
conversion moved in the export message (a fraction of a percent: about
0.1 percent on a real duct piece, far less on a simple rim), and judges
the file at the mesh tolerance of 1 percent. Offsets of lines
and arcs simplify to lines and arcs and need no conversion; STL and 3MF
carry offset geometry as triangles and need none either.

```python
from build123d import *

with BuildPart() as flange:
    with BuildSketch():
        Ellipse(20, 12)
        offset(amount=3)  # the rim is an offset curve
    extrude(amount=10)
```

Exporting this part as STEP reports the conversion; as STL or 3MF it needs
none.

API evidence: the installed build123d 0.11.1 implementations of `Helix`,
`Polygon`, `RegularPolygon`, `sweep`, `Box`, and `Cylinder`, plus the bundled
[operations](../../docs/build123d/operations.md) and
[topology selection](../../docs/build123d/topology_selection.md) references.
