<!-- Source: https://build123d.readthedocs.io/en/latest/key_concepts.html -->

> Adapted from the [build123d documentation](https://build123d.readthedocs.io/en/latest/key_concepts.html), Copyright 2022 Gumyr, under the [Apache License 2.0](LICENSE). Converted to Markdown and edited for CADmark by BFJ Concerns; see [NOTICE](../../NOTICE).

Key Concepts
============

The following key concepts will help new users understand build123d quickly.

## Topology

Topology, in the context of 3D modeling and computational geometry, is the branch of mathematics that deals with the properties and relationships of geometric objects that are preserved under continuous deformations. In the context of CAD and modeling software like build123d, topology refers to the hierarchical structure of geometric elements (vertices, edges, faces, etc.) and their relationships in a 3D model. This structure defines how the components of a model are connected, enabling operations like Boolean operations, transformations, and analysis of complex shapes. Topology provides a formal framework for understanding and manipulating geometric data in a consistent and reliable manner.

The following are the topological objects that compose build123d objects:

### Vertex

A Vertex is a data structure representing a 0D topological element. It defines a precise point in 3D space, often at the endpoints or intersections of edges in a 3D model. These vertices are part of the topological structure used to represent complex shapes in build123d.

### Edge

An Edge in build123d is a fundamental geometric entity representing a 1D element in a 3D model. It defines the shape and position of a 1D curve within the model. Edges play a crucial role in defining the boundaries of faces and in constructing complex 3D shapes.

### Wire

A Wire in build123d is a topological construct that represents a connected sequence of Edges, forming a 1D closed or open loop within a 3D model. Wires define the boundaries of faces and can be used to create complex shapes, making them essential for modeling in build123d.

### Face

A Face in build123d represents a 2D surface in a 3D model. It defines the boundary of a region and can have associated geometric and topological data. Faces are vital for shaping solids, providing surfaces where other elements like edges and wires are connected to form complex structures.

### Shell

A Shell in build123d represents a collection of Faces, defining a closed, connected volume in 3D space. It acts as a container for organizing and grouping faces into a single shell, essential for defining complex 3D shapes like solids or assemblies within the build123d modeling framework.

### Solid

A Solid in build123d is a 3D geometric entity that represents a bounded volume with well-defined interior and exterior surfaces. It encapsulates a closed and watertight shape, making it suitable for modeling solid objects and enabling various Boolean operations such as union, intersection, and subtraction.

### Compound

A Compound in build123d is a container for grouping multiple geometric shapes. It can hold various types of entities, such as vertices, edges, wires, faces, shells, or solids, into a single structure. This makes it a versatile tool for managing and organizing complex assemblies or collections of shapes within a single container.

### Shape

A Shape in build123d represents a fundamental building block in 3D modeling. It encompasses various topological elements like vertices, edges, wires, faces, shells, solids, and compounds. The Shape class is the base class for all of the above topological classes.

One can use the `show_topology()` method to display the topology of a shape as shown here for a unit cube:

```
Solid                      at 0x7f94c55430f0, Center(0.5, 0.5, 0.5)
└── Shell                  at 0x7f94b95835f0, Center(0.5, 0.5, 0.5)
├── Face               at 0x7f94b95836b0, Center(0.0, 0.5, 0.5)
│   └── Wire           at 0x7f94b9583730, Center(0.0, 0.5, 0.5)
│       ├── Edge       at 0x7f94b95838b0, Center(0.0, 0.0, 0.5)
│       │   ├── Vertex at 0x7f94b9583470, Center(0.0, 0.0, 1.0)
│       │   └── Vertex at 0x7f94b9583bb0, Center(0.0, 0.0, 0.0)
│       ├── Edge       at 0x7f94b95838a30, Center(0.0, 0.5, 1.0)
│       │   ├── Vertex at 0x7f94b9583030, Center(0.0, 1.0, 1.0)
│       │   └── Vertex at 0x7f94b9583e70, Center(0.0, 0.0, 1.0)
...
```

Users of build123d will often reference topological objects as part of the process of creating the object as described below.

## Coordinate Systems

In build123d, coordinate systems and reference frames play an important role in positioning and orienting geometric objects. Two main concepts are used: workplanes and locations.

### Workplane

A Workplane is a 2D coordinate system within 3D space that defines the plane on which geometry is constructed. It has an origin, and an x-axis and y-axis that lie within the plane. The z-axis is perpendicular to the plane, pointing upward. The Workplane contains the plane along with a collection of mode-dependent geometric entities used to construct geometry.

### Location

A Location represents a 3D coordinate system, including both position (origin) and orientation (rotation). Locations are used to position and orient geometric objects in 3D space.

## Ownership

In CAD, ownership refers to the relationship between geometric elements and the object that contains or "owns" them. For example, when a user creates a box as part of a BuildPart, the BuildPart owns the box. When the user creates a new object from the box through Boolean operations or other transformations, the old object may be replaced by the new object as the "current" object.

## Builder Mode

CADmark uses build123d through builder contexts such as `BuildPart`,
`BuildSketch`, and `BuildLine`.

In builder mode, a stateful context accumulates geometry as operations are
performed. Each operation modifies the object within the active context, and
the final model is typically accessed through the builder variable such as
`part.part`.
