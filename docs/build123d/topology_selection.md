<!-- Source: https://build123d.readthedocs.io/en/latest/topology_selection.html -->

> Adapted from the [build123d documentation](https://build123d.readthedocs.io/en/latest/topology_selection.html), Copyright 2022 Gumyr, under the [Apache License 2.0](LICENSE). Converted to Markdown and edited for CADmark by BFJ Concerns; see [NOTICE](../../NOTICE).

# Topology Selection and Exploration

Topology is the structure of build123d geometric features. Traversing the topology of a part is often required to specify objects for an operation or to locate a CAD feature. Selectors allow selection of topology objects into a ShapeList. Operators are powerful methods that further explore and refine a ShapeList for subsequent operations.

## Selectors

Selectors provide methods to extract all or a subset of a feature type in the referenced object. These methods select Edges, Faces, Solids, Vertices, or Wires in Builder objects or from Shape objects themselves. All of these methods return a ShapeList, which is a subclass of `list` and may be sorted, grouped, or filtered by operators.

### Selector Overview

| Selector | Criteria | Applicability | Description |
|----------|----------|---------------|-------------|
| vertices | ALL, LAST | BuildLine, BuildSketch, BuildPart | Vertex extraction |
| edges | ALL, LAST, NEW | BuildLine, BuildSketch, BuildPart | Edge extraction |
| wires | ALL, LAST | BuildLine, BuildSketch, BuildPart | Wire extraction |
| faces | ALL, LAST | BuildSketch, BuildPart | Face extraction |
| solids | ALL, LAST | BuildPart | Solid extraction |

Both shape objects and builder objects have access to selector methods.

#### In Context (Builder)

```build123d
with BuildSketch() as context:
    Rectangle(1, 1)
    context.edges()
    edges()  # Build context implicitly has access
```

#### Out of Context (Shape)

```build123d
context.sketch.edges()
Rectangle(1, 1).edges()
```

### Select Criteria

#### Select.ALL (Default)

Select all features of a type:

```build123d
with BuildPart() as part:
    Box(5, 5, 1)
    Cylinder(1, 5)

    part.vertices()  # Same as part.vertices(Select.ALL)
    part.edges()
    part.faces()
```

#### Select.LAST

Select features created or altered by the most recent operation:

```build123d
with BuildPart() as part:
    Box(5, 5, 1)
    Cylinder(1, 5)

    part.vertices(Select.LAST)
    part.edges(Select.LAST)
    part.faces(Select.LAST)
```

#### Select.NEW (Edges Only)

Select only edges created in the last operation which neither existed in the referenced object before the operation, nor existed in the modifying object:

```build123d
with BuildPart() as part:
    Box(5, 5, 1)
    Cylinder(1, 5)

    part.edges(Select.NEW)
```

**Important Note:** `Select` as selector criteria is only valid for builder objects. It does not work out of context with shape objects.

### New Edges in Algebra Mode

The utility method `new_edges` compares one or more shape objects to another "combined" shape object and returns the edges new to the combined shape. `new_edges` is necessary in Algebra Mode where `Select.NEW` is unavailable:

```build123d
box = Box(5, 5, 1)
circle = Cylinder(2, 5)
part = box + circle
edges = new_edges(box, circle, combined=part)
```

You can also find edges created during chamfer or fillet operations:

```build123d
box = Box(5, 5, 1)
circle = Cylinder(2, 5)
part_before = box + circle
edges = part_before.edges().filter_by(lambda a: a.length == 1)
part = fillet(edges, 1)
edges = new_edges(part_before, combined=part)
```

## Operators

Operators provide methods to refine a ShapeList of features isolated by a selector to further specify feature(s). These methods can sort, group, or filter ShapeList objects and return a modified ShapeList, or in the case of `group_by`, a `GroupBy` list of ShapeList objects.

### Operator Overview

| Method | Criteria | Description |
|--------|----------|-------------|
| sort_by | Axis, Edge, Wire, SortBy, callable, property | Sort ShapeList by criteria |
| sort_by_distance | Shape, VectorLike | Sort ShapeList by distance from criteria |
| group_by | Axis, Edge, Wire, SortBy, callable, property | Group ShapeList by criteria |
| filter_by | Axis, Plane, GeomType, ShapePredicate, property | Filter ShapeList by criteria |
| filter_by_position | Axis | Filter ShapeList by Axis and min/max values |

Operator methods take various criteria to refine ShapeList:
- Geometric objects: Axis, Plane
- Topological objects: Edge, Wire
- Enums: SortBy, GeomType
- Properties: Face.area, Edge.length
- ShapePredicate: lambda functions like `lambda e: e.is_interior == 1`
- Callable: Vertex().distance

### Sort

A ShapeList can be sorted with the `sort_by` and `sort_by_distance` methods based on sorting criteria. Sorting is critical when isolating individual features as a ShapeList from a selector is typically unordered.

#### Sort by Axis

Capture vertices furthest along X:

```build123d
part.vertices().sort_by(Axis.X)[-4:]
```

#### Sort by Distance

Sort features by distance from a reference point:

```build123d
part.vertices().sort_by_distance(reference_point)
```

#### Sort by SortBy Enum

Sort by various properties:
- SortBy.AREA
- SortBy.LENGTH
- SortBy.DISTANCE

### Group

A ShapeList can be grouped with the `group_by` method based on grouping criteria. Unlike sort, `group_by` returns a `GroupBy` object - a list of ShapeList objects sorted by the grouping criteria.

GroupBy can be:
- Printed to view members of each group
- Indexed like a list to retrieve a ShapeList
- Accessed using a key with the `group` method

```build123d
# Get edges from the smallest faces by area
part.faces().group_by(SortBy.AREA)[0].edges()
```

### Filter

A ShapeList can be filtered with the `filter_by` and `filter_by_position` methods based on filtering criteria. Filters are a flexible way to isolate (or exclude) features based on known criteria.

#### Filter by Geometric Property

Find all faces with a normal in the +Z direction:

```build123d
part.faces().filter_by(lambda f: f.normal_at() == Vector(0, 0, 1))
```

#### Filter by GeomType

Filter by geometry type (e.g., line, circle, plane):

```build123d
edges.filter_by(GeomType.LINE)
```

#### Filter by Axis and Plane

Filter faces parallel to specific axes or planes:

```build123d
part.faces().filter_by(Axis.Z)
part.faces().filter_by(Plane.XY)
```

#### Filter by Position

Filter by minimum or maximum position along an axis:

```build123d
part.faces().filter_by_position(Axis.X)
```

#### Complex Filters

Combine multiple filters for precise feature selection:

```build123d
part.faces().filter_by(
    lambda f: f.area > 10 and f.normal_at().z == 1
)
```

## ShapeList

ShapeList is a subclass of `list`, so standard list operations apply:

```build123d
edges = part.edges()
edges[-4:]  # Last 4 edges
edges[0]    # First edge
len(edges)  # Count
```

Selectors and operators can be chained for complex queries:

```build123d
part.vertices()
    .sort_by(Axis.X)
    .filter_by(lambda v: v.x > 0)
    .group_by(SortBy.DISTANCE)
```
