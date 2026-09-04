# A profile revolved about an axis

A body of revolution is a half-section sketched against the axis and
swept round it. The section is drawn in a plane that contains the axis,
and every radius in it is derived from one outer diameter.

```python
from build123d import *

# Parameters
outer_diameter = 40.0
bore_diameter = 12.0
height = 30.0
wall = (outer_diameter - bore_diameter) / 2
revolve_angle = 360.0

outer_radius = outer_diameter / 2
bore_radius = bore_diameter / 2

# Geometry
with BuildPart() as bushing:
    with BuildSketch(Plane.XZ) as section:
        with Locations(((bore_radius + outer_radius) / 2, height / 2)):
            Rectangle(wall, height)
    revolve(axis=Axis.Z, revolution_arc=revolve_angle)
```
