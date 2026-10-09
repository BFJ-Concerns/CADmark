<!-- Source: https://build123d.readthedocs.io/en/latest/tips.html -->

> Adapted from the [build123d documentation](https://build123d.readthedocs.io/en/latest/tips.html), Copyright 2022 Gumyr, under the [Apache License 2.0](LICENSE). Converted to Markdown and edited for CADmark by BFJ Concerns; see [NOTICE](../../NOTICE).

# Tips, Best Practices and FAQ

## Can't Get There from Here

Not all parts designed with build123d can be successfully constructed by the underlying CAD core. Designers may need to explore alternative approaches. For example, if a multi-section sweep fails, a loft operation might work instead. "CAD is a complex field and patience may be required to achieve the desired results."

## 2D before 3D

When building intricate 3D objects, start with 2D work before progressing to 3D. This approach works well because 3D operations are slower and more error-prone. The typical workflow applies operations like extrude or revolve to 2D sketches:

```python
with BuildPart() as my_part:
    with BuildSketch() as part_profile:
        ...
    extrude(amount=some_distance)
    ...
```

The sketch can include many combined and modified objects before extrusion.

## Delay Chamfers and Fillets

Chamfers and fillets increase design complexity by converting simple vertices and edges into arcs or non-planar faces. "It's recommended to perform these operations towards the end of the object's design." Whilst 2D chamfers and fillets are generally safer, prioritise them last in 3D workflows.

## Parameterize

build123d excels at creating fully parameterised parts. Using variables for critical dimensions and deriving others from key parameters enables creating multiple part variations. "When inevitable change requests arise, a simple parameter adjustment may be all that's required."

## Use Shallow Copies

Shallow copies of repeated components—fasteners, bearings, chain links—significantly improve performance. Note that "if one instance of the object changes all will change."

## Object Selection

Select features from higher-level topology first. For example, select the face containing edges before selecting individual edges to chamfer.

```python
top_face: Face = plate.faces().sort_by(Axis.Z)[-1]
hole_edges = top_face.edges().filter_by(GeomType.CIRCLE)
chamfer(hole_edges, length=1)
```

## build123d - CadQuery Integration

Both libraries share the OpenCascade wrapper (OCP), allowing object interchange via the `wrapped` attribute:

```python
# CadQuery to build123d
b3d_solid.wrapped = cq_solid.wrapped

# build123d to CadQuery
cq_solid.wrapped = b123d_box.part.solid().wrapped
```

## Self Intersection

Avoid creating self-intersecting objects, even at single vertices, as these topologies become invalid. Split helical shapes like screw threads into multiple sections stored in an assembly instead.

## Packing Objects on a Plane

The `pack.pack()` function translates shapes to prevent overlapping. Use the `align_z` argument to align all objects to the zero Z coordinate, helpful for "preparing print setups for 3D printing."

## Glob Imports in build123d

Whilst `from build123d import *` violates typical software practices, it's appropriate here. build123d functions as a "Domain-Specific Language which acts as the user interface for a CAD application." In this context, prioritising developer ergonomics in REPL environments outweighs naming convention concerns.

## BuildSketch Plane Issues

All sketches are created on a local Plane.XY regardless of specified planes. Sorting operations use global coordinates, making them unreliable for non-aligned planes. Work on Plane.XY within sketches and let the system handle final plane placement.

## BuildLine Within BuildSketch

When nesting `BuildLine` inside `BuildSketch`, keep `BuildLine` on Plane.XY. Specifying a different plane causes reorientation to Plane.XY, potentially producing unexpected results.

## Builder Coordinate System Inheritance

Nested Builders don't inherit workplanes from parent Builders. Each Builder either uses a user-provided workplane or defaults to Plane.XY. This prevents confusion from nested scope changes.
