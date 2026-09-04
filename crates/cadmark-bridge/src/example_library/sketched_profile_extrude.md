# A sketched profile, extruded into a plate

The profile is drawn on a plane, then given thickness. Dimensions are
named at the top and the ones that depend on others are derived, so a
later request to widen the plate moves the mounting holes with it.

```python
from build123d import *

# Parameters
plate_width = 80.0
plate_depth = 50.0
plate_thickness = 6.0
corner_radius = 8.0
hole_diameter = 5.0
hole_inset = corner_radius  # holes sit under the rounded corners

# Geometry
with BuildPart() as plate:
    with BuildSketch(Plane.XY) as profile:
        RectangleRounded(plate_width, plate_depth, corner_radius)
        with Locations(
            (plate_width / 2 - hole_inset, plate_depth / 2 - hole_inset),
            (-plate_width / 2 + hole_inset, plate_depth / 2 - hole_inset),
            (plate_width / 2 - hole_inset, -plate_depth / 2 + hole_inset),
            (-plate_width / 2 + hole_inset, -plate_depth / 2 + hole_inset),
        ):
            Circle(hole_diameter / 2, mode=Mode.SUBTRACT)
    extrude(amount=plate_thickness)
```
