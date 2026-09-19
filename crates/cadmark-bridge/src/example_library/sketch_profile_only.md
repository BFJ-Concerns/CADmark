# A profile drawn as a sketch, before it becomes a solid

A design can stop at a sketch: the profile is drawn, shown flat on its
plane, and the user can point at its curves, corners and regions or
export it as an SVG or DXF drawing. The sketch is the script's result
when nothing has yet extruded or revolved it. Draw with the stock sketch
objects and operations — `chamfer` and `offset` keep each curve's origin,
`fillet` and `make_face` rebuild the outline — and give it thickness in a
later step when the profile is agreed.

```python
from build123d import *

# Parameters
bracket_width = 60.0
bracket_height = 40.0
web_thickness = 8.0
corner_chamfer = web_thickness / 2
mount_hole_diameter = 5.0
mount_hole_spacing = bracket_width - 2 * web_thickness  # holes clear the web

# Geometry
with BuildSketch(Plane.XZ) as profile:
    # An L-shaped web, drawn as a rectangle with the inner corner cut away
    Rectangle(bracket_width, bracket_height, align=(Align.MIN, Align.MIN))
    with Locations((web_thickness, web_thickness)):
        Rectangle(
            bracket_width - web_thickness,
            bracket_height - web_thickness,
            align=(Align.MIN, Align.MIN),
            mode=Mode.SUBTRACT,
        )
    chamfer(profile.vertices().group_by(Axis.X)[-1], corner_chamfer)
    with Locations(
        (bracket_width / 2 - mount_hole_spacing / 2, web_thickness / 2),
        (bracket_width / 2 + mount_hole_spacing / 2, web_thickness / 2),
    ):
        Circle(mount_hole_diameter / 2, mode=Mode.SUBTRACT)
```
