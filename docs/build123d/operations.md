<!-- Source: https://build123d.readthedocs.io/en/latest/operations.html -->

# Operations

Operations are functions that take objects as inputs and transform them into new objects. For example, a 2D Sketch can be extruded to create a 3D Part. All operations are Python functions which can be applied using both the Algebra and Builder APIs.

**Important:** Objects created by operations are not affected by `Locations`, meaning their position is determined solely by the input objects used in the operation.

## Builder API Example

```build123d
with BuildPart() as cylinder:
    with BuildSketch():
        Circle(radius)
    extrude(amount=height)
```

## Algebra API Example

```build123d
cylinder = extrude(Circle(radius), amount=height)
```

## Available Operations

The following table summarises all available operations. Operations marked as 1D are applicable to BuildLine and Algebra Curve, 2D to BuildSketch and Algebra Sketch, 3D to BuildPart and Algebra Part.

| Operation | Description | 1D | 2D | 3D |
|-----------|-------------|----|----|-----|
| add | Add object to builder | ✓ | ✓ | ✓ |
| bounding_box | Add bounding box as Shape | ✓ | ✓ | ✓ |
| chamfer | Bevel Vertex or Edge | - | ✓ | ✓ |
| draft | Add a draft taper to a part | - | - | ✓ |
| extrude | Draw 2D Shape into 3D | - | - | ✓ |
| fillet | Radius Vertex or Edge | - | ✓ | ✓ |
| full_round | Round-off Face along given Edge | - | ✓ | - |
| loft | Create 3D Shape from sections | - | - | ✓ |
| make_brake_formed | Create sheet metal parts | - | - | ✓ |
| make_face | Create a Face from Edges | - | ✓ | - |
| make_hull | Create Convex Hull from Edges | - | ✓ | - |
| mirror | Mirror about Plane | ✓ | ✓ | ✓ |
| offset | Inset or outset Shape | ✓ | ✓ | ✓ |
| project | Project points, lines or Faces | ✓ | ✓ | - |
| project_workplane | Create workplane for projection | - | - | - |
| revolve | Swing 2D Shape about Axis | - | - | ✓ |
| scale | Change size of Shape | ✓ | ✓ | ✓ |
| section | Generate 2D slices from 3D Shape | - | - | ✓ |
| split | Divide object by Plane | ✓ | ✓ | ✓ |
| sweep | Extrude 1/2D section(s) along path | - | ✓ | ✓ |
| thicken | Expand 2D section(s) | - | - | ✓ |
| trace | Convert lines to faces | - | ✓ | - |

## Selectors in Builder Context

Selectors extract objects from the builder currently within scope without explicit reference:

| Selector | Description | Line | Sketch | Part |
|----------|-------------|------|--------|------|
| edge | Select edge from current builder | ✓ | ✓ | ✓ |
| edges | Select edges from current builder | ✓ | ✓ | ✓ |
| face | Select face from current builder | - | ✓ | ✓ |
| faces | Select faces from current builder | - | ✓ | ✓ |
| solid | Select solid from current builder | - | - | ✓ |
| solids | Select solids from current builder | - | - | ✓ |
| vertex | Select vertex from current builder | ✓ | ✓ | ✓ |
| vertices | Select vertices from current builder | ✓ | ✓ | ✓ |
| wire | Select wire from current builder | ✓ | ✓ | ✓ |
| wires | Select wires from current builder | ✓ | ✓ | ✓ |

## Common 3D Operations

### extrude
Draw a 2D Shape into a 3D solid by extending it along an axis.

### revolve
Rotate a 2D shape around an axis to create a 3D solid of revolution.

### loft
Create a 3D form by interpolating between multiple cross-sections.

### sweep
Extrude a 2D or 3D section along a path to create a complex 3D shape.

### draft
Apply a taper angle to the faces of a part for manufacturing purposes.

### fillet
Radius a vertex or edge to create smooth transitions.

### chamfer
Bevel a vertex or edge with a flat transition.

### mirror
Mirror the shape about a specified plane.

### offset
Inset or outset a shape by a specified distance.

### section
Generate 2D cross-sections from a 3D shape at specified locations.

### split
Divide an object by a plane into separate parts.

### thicken
Expand a 2D face into a 3D solid with a specified thickness.

### scale
Change the size of a shape by a scaling factor.

### bounding_box
Create a bounding box around the shape.

### add
Add an object to the current builder.

## Common 2D Operations

### make_face
Create a face from closed edges.

### make_hull
Create a convex hull from edges.

### sweep
Extrude a section along a path in 2D.

### full_round
Round off a face along a given edge.

### trace
Convert line segments to filled faces.
