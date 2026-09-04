# The same work in algebra mode

Algebra mode builds the same part with operators instead of a builder
context: `+` fuses, `-` cuts, `*` intersects, and a location multiplies
onto a shape to place it. Either idiom is a full citizen of build123d;
this one often reads well when the part is a short chain of solids.

```python
from build123d import *

# Parameters
body_diameter = 30.0
body_height = 25.0
flange_diameter = body_diameter * 1.6
flange_thickness = 5.0
bore_diameter = 10.0

# Geometry
flange = Cylinder(flange_diameter / 2, flange_thickness)
barrel = Pos(0, 0, (flange_thickness + body_height) / 2) * Cylinder(
    body_diameter / 2, body_height
)
bore = Cylinder(bore_diameter / 2, flange_thickness + body_height)
bore = Pos(0, 0, (flange_thickness + body_height) / 2 - flange_thickness / 2) * bore

part = (flange + barrel) - bore
```
