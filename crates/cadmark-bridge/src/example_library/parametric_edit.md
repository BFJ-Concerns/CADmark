# Editing an existing script: naming the route first

A follow-up request usually touches something that exists at two levels —
the sketch that drew a profile, and the solid the profile became. Say
which one the edit changes before changing it, and extend the parameter
block rather than replacing a derived value with a literal.

Starting script:

```python
from build123d import *

# Parameters
plate_width = 60.0
plate_depth = 40.0
plate_thickness = 5.0

with BuildPart() as plate:
    with BuildSketch(Plane.XY):
        Rectangle(plate_width, plate_depth)
    extrude(amount=plate_thickness)
```

Request: *"round these corners"*, pointing at a vertical edge of the plate.

The corner exists as a sketch vertex and as a vertical edge of the solid.
Rounding the sketch corner carries through to anything later extruded from
that profile; rounding the solid edge affects only this solid. Here the
profile is the source of the shape, so the edit goes to the sketch — and
the reply says so.

```python
from build123d import *

# Parameters
plate_width = 60.0
plate_depth = 40.0
plate_thickness = 5.0
corner_radius = 6.0  # added for this edit

with BuildPart() as plate:
    with BuildSketch(Plane.XY):
        RectangleRounded(plate_width, plate_depth, corner_radius)
    extrude(amount=plate_thickness)
```
