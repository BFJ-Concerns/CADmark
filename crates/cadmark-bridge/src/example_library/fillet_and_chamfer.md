# Filleting and chamfering, once the topology is settled

Edge treatments come last, after the shape they round exists, and the
edges are found from the feature down — the face first, then its edges —
so an earlier change does not silently move the fillet elsewhere.

```python
from build123d import *

# Parameters
base_length = 50.0
base_width = 50.0
base_height = 15.0
edge_radius = 4.0
chamfer_size = edge_radius / 2

# Geometry
with BuildPart() as bracket:
    Box(base_length, base_width, base_height)
    # Round the four vertical edges
    fillet(bracket.edges().filter_by(Axis.Z), radius=edge_radius)
    # Break the top rim
    top_face = bracket.faces().sort_by(Axis.Z)[-1]
    chamfer(top_face.edges(), length=chamfer_size)
```
