<!-- Source: https://build123d.readthedocs.io/en/latest/key_concepts_builder.html -->

> Adapted from the [build123d documentation](https://build123d.readthedocs.io/en/latest/key_concepts_builder.html), Copyright 2022 Gumyr, under the [Apache License 2.0](LICENSE). Converted to Markdown and edited for CADmark by BFJ Concerns; see [NOTICE](../../NOTICE).

# Key Concepts (Builder Mode)

Builder mode provides a stateful approach to building CAD models using context managers like `BuildPart`, `BuildSketch`, and `BuildLine`. Each context accumulates geometry as operations are performed.

## BuildPart

`BuildPart` is the primary builder context for constructing 3D solid models. It maintains:

- A current workplane for reference
- A collection of solid shapes being constructed
- A face/edge selection tracking system

Within a `BuildPart` context, you can:

- Create primitive shapes (Box, Cylinder, Sphere, etc.)
- Apply operations (Fillet, Chamfer, etc.)
- Perform Boolean operations (union, cut, intersection)
- Select and modify faces and edges

Example:
```python
from build123d import *

with BuildPart() as bp:
    Box(10, 20, 30)
    # The box is now part of the BuildPart
```

## BuildSketch

`BuildSketch` is the builder context for constructing 2D sketches. It:

- Provides a 2D drawing surface (workplane)
- Accumulates 2D geometry (lines, circles, rectangles, etc.)
- Supports constraints and geometric operations
- Can be extruded or revolved into 3D shapes

Example:
```python
with BuildSketch() as bs:
    Rectangle(10, 20)
    Circle(radius=5)
```

## BuildLine

`BuildLine` is used to construct 1D curves (paths). It's useful for:

- Creating complex paths for sweeps or lofts
- Building curves that can be used as guides
- Defining construction geometry

## Workplane Management

In builder mode, each context maintains a current workplane. You can:

- Change the active workplane with operations like `workplane()`
- Use `Plane` to specify custom workplanes
- Switch between predefined planes (XY, YZ, XZ)

## Chaining Operations

Builder mode supports natural chaining of operations:

```python
with BuildPart() as bp:
    Box(10, 10, 10)
    # Current part is the box
    Fillet(edges=bp.edges(), radius=1)
    # Fillet applied to box edges
```

## Practical Example

Here's a practical example combining multiple builder features:

```python
from build123d import *

# Create a simple bracket
with BuildPart() as bp:
    # Create base plate
    with BuildSketch() as bs:
        Rectangle(50, 40)
    Extrude(amount=5)

    # Add mounting holes
    with BuildSketch() as bs:
        Circle(5)
    Hole(depth=5)
```
