<!-- Source: https://build123d.readthedocs.io/en/latest/key_concepts_algebra.html -->

# Key Concepts (Algebra Mode)

Algebra mode provides a functional approach to CAD modeling where operations return new shapes rather than modifying existing ones. This approach emphasizes functional composition and immutability.

## Shape Operations

In algebra mode, shapes are combined using standard Python operators:

### Union (+)

Combines two shapes into a single shape:

```python
from build123d import *

box = Box(10, 10, 10)
sphere = Sphere(radius=6)
combined = box + sphere
```

### Cut (-)

Subtracts one shape from another:

```python
box = Box(10, 10, 10)
hole = Sphere(radius=3)
result = box - hole
```

### Intersect (*)

Creates the intersection of two shapes:

```python
box1 = Box(10, 10, 10)
box2 = Box(8, 8, 8)
intersection = box1 * box2
```

## Advantages of Algebra Mode

- **Functional composition**: Chain operations naturally
- **Immutability**: Original shapes are preserved
- **Flexibility**: Mix and match operations in any order
- **Readability**: Expressions read like mathematics

## Example: Creating Complex Geometry

```python
from build123d import *

# Create a block with a spherical cavity
block = Box(20, 20, 20)
cavity = Sphere(radius=8)
result = block - cavity

# Add mounting features
mount_pad = Box(5, 5, 10)
mount_hole = Cylinder(radius=1, height=10)
mounting = mount_pad - mount_hole

# Combine components
assembly = result + mounting
```

## Mixing Builder and Algebra Modes

build123d allows seamless mixing of both modes:

```python
with BuildPart() as bp:
    # Start with builder mode
    Box(10, 10, 10)

    # Get the current shape
    current = bp.part

    # Use algebra operations
    enhanced = current + Sphere(radius=5)

    # Switch back to builder mode
    bp.part = enhanced
```

## Working with Locations in Algebra Mode

Shapes can be positioned and oriented using `Location`:

```python
from build123d import *

box = Box(10, 10, 10)
sphere = Sphere(radius=5)

# Position sphere at specific location
loc = Location((0, 0, 20))
positioned_sphere = sphere @ loc

# Combine
result = box + positioned_sphere
```
