# Cutting material away: a pocket and a through hole

Material is removed by a second operation on the finished solid, not by
drawing round the void. `Mode.SUBTRACT` cuts a sketched shape; `Hole`
and `CounterBoreHole` cut a drilled feature positioned on a face.

```python
from build123d import *

# Parameters
block_length = 60.0
block_width = 40.0
block_height = 20.0
pocket_margin = 8.0
pocket_depth = block_height / 2
bore_diameter = 8.0

pocket_length = block_length - 2 * pocket_margin
pocket_width = block_width - 2 * pocket_margin

# Geometry
with BuildPart() as body:
    Box(block_length, block_width, block_height)
    # Pocket in the top face
    top = body.faces().sort_by(Axis.Z)[-1]
    with BuildSketch(Plane(top)) as pocket:
        Rectangle(pocket_length, pocket_width)
    extrude(amount=-pocket_depth, mode=Mode.SUBTRACT)
    # Through hole on the axis
    with Locations(body.faces().sort_by(Axis.Z)[-1]):
        Hole(radius=bore_diameter / 2)
```
