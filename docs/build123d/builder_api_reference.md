<!-- Source: https://build123d.readthedocs.io/en/latest/builder_api_reference.html -->

# Builder Common API Reference

The following are common to all the builders.

## Selector Methods

**
Builder.vertices(*select: ~build123d.build_enums.Select = *) → ShapeList[Vertex][source]**

Return Vertices

Return either all or the vertices created during the last operation.

**Parameters:**

**select** (*Select**, **optional*) – Vertex selector. Defaults to Select.ALL.

**Returns:**

Vertices extracted

**Return type:**

*ShapeList*[*Vertex*]

**
Builder.faces(*select: ~build123d.build_enums.Select = *) → ShapeList[Face][source]**

Return Faces

Return either all or the faces created during the last operation.

**Parameters:**

**select** (*Select**, **optional*) – Face selector. Defaults to Select.ALL.

**Returns:**

Faces extracted

**Return type:**

*ShapeList*[*Face*]

**
Builder.edges(*select: ~build123d.build_enums.Select = *) → ShapeList[Edge][source]**

Return Edges

Return either all or the edges created during the last operation.

**Parameters:**

**select** (*Select**, **optional*) – Edge selector. Defaults to Select.ALL.

**Returns:**

Edges extracted

**Return type:**

*ShapeList*[*Edge*]

**
Builder.wires(*select: ~build123d.build_enums.Select = *) → ShapeList[Wire][source]**

Return Wires

Return either all or the wires created during the last operation.

**Parameters:**

**select** (*Select**, **optional*) – Wire selector. Defaults to Select.ALL.

**Returns:**

Wires extracted

**Return type:**

*ShapeList*[*Wire*]

**
Builder.solids(*select: ~build123d.build_enums.Select = *) → ShapeList[Solid][source]**

Return Solids

Return either all or the solids created during the last operation.

**Parameters:**

**select** (*Select**, **optional*) – Solid selector. Defaults to Select.ALL.

**Returns:**

Solids extracted

**Return type:**

*ShapeList*[*Solid*]

## Enums

**
*class *Align(*value*)[source]**

Align object about Axis

**
CENTER* = 2***

**
MAX* = 3***

**
MIN* = 1***

**
NONE* = None***

**
*class *CenterOf(*value*)[source]**

Center Options

**
BOUNDING_BOX* = 3***

**
GEOMETRY* = 1***

**
MASS* = 2***

**
*class *FontStyle(*value*)[source]**

Text Font Styles

**
BOLD* = 2***

**
BOLDITALIC* = 4***

**
ITALIC* = 3***

**
REGULAR* = 1***

**
*class *GeomType(*value*)[source]**

CAD geometry object type

**
BEZIER* = 6***

**
BSPLINE* = 7***

**
CIRCLE* = 12***

**
CONE* = 3***

**
CYLINDER* = 2***

**
ELLIPSE* = 13***

**
EXTRUSION* = 9***

**
HYPERBOLA* = 14***

**
LINE* = 11***

**
OFFSET* = 10***

**
OTHER* = 16***

**
PARABOLA* = 15***

**
PLANE* = 1***

**
REVOLUTION* = 8***

**
SPHERE* = 4***

**
TORUS* = 5***

**
*class *Keep(*value*)[source]**

Split options

**
ALL* = 1***

**
BOTH* = 3***

**
BOTTOM* = 2***

**
INSIDE* = 4***

**
OUTSIDE* = 5***

**
TOP* = 6***

**
*class *Kind(*value*)[source]**

Offset corner transition

**
ARC* = 1***

**
INTERSECTION* = 2***

**
TANGENT* = 3***

**
*class *Mode(*value*)[source]**

Combination Mode

**
ADD* = 1***

**
INTERSECT* = 3***

**
PRIVATE* = 5***

**
REPLACE* = 4***

**
SUBTRACT* = 2***

**
*class *Select(*value*)[source]**

Selector scope - all, last operation or new objects

**
ALL* = 1***

**
LAST* = 2***

**
NEW* = 3***

**
*class *SortBy(*value*)[source]**

Sorting criteria

**
AREA* = 3***

**
DISTANCE* = 5***

**
LENGTH* = 1***

**
RADIUS* = 2***

**
VOLUME* = 4***

**
*class *Transition(*value*)[source]**

Sweep discontinuity handling option

**
RIGHT* = 1***

**
ROUND* = 2***

**
TRANSFORMED* = 3***

**
*class *Until(*value*)[source]**

Extrude limit

**
FIRST* = 4***

**
LAST* = 2***

**
NEXT* = 1***

**
PREVIOUS* = 3***

## Locations

**
*class *Locations(**pts: Vector | tuple[float, float] | tuple[float, float, float] | Sequence[float] | Vertex | Location | Face | Plane | Axis | Iterable[Vector | tuple[float, float] | tuple[float, float, float] | Sequence[float] | Vertex | Location | Face | Plane | Axis]*)[source]**

Location Context: Push Points

Creates a context of locations for Part or Sketch

**Parameters:**

**pts** (*Union**[**VectorLike**, **Vertex**, **Location**, **Face**, **Plane**, **Axis**] or **iterable** of **same*) – sequence of points to push

**Variables:**

**local_locations** (*list{Location}*) – locations relative to workplane

**
local_locations**

values independent of workplanes

**
*class *GridLocations(*x_spacing: float*, *y_spacing: float*, *x_count: int*, *y_count: int*, *align: ~build123d.build_enums.Align | tuple[~build123d.build_enums.Align*, *~build123d.build_enums.Align] = (*, *)*)[source]**

Location Context: Rectangular Array

Creates a context of rectangular array of locations for Part or Sketch

**Parameters:**

- 
**x_spacing** (*float*) – horizontal spacing

- 
**y_spacing** (*float*) – vertical spacing

- 
**x_count** (*int*) – number of horizontal points

- 
**y_count** (*int*) – number of vertical points

- 
**align** (*Union**[**Align**, **tuple**[**Align**, **Align**]**]**, **optional*) – align min, center, or max of object.
Defaults to (Align.CENTER, Align.CENTER).

**Variables:**

- 
**x_spacing** (*float*) – horizontal spacing

- 
**y_spacing** (*float*) – vertical spacing

- 
**x_count** (*int*) – number of horizontal points

- 
**y_count** (*int*) – number of vertical points

- 
**align** (*Union**[**Align**, **tuple**[**Align**, **Align**]**]*) – align min, center, or max of object.

- 
**local_locations** (*list{Location}*) – locations relative to workplane

**Raises:**

**ValueError** – Either x or y count must be greater than or equal to one.

**
local_locations**

values independent of workplanes

**
max**

top right corner

**
min**

bottom left corner

**
size**

size of the grid

**
*class *HexLocations(*radius: float*, *x_count: int*, *y_count: int*, *major_radius: bool = False*, *align: ~build123d.build_enums.Align | tuple[~build123d.build_enums.Align*, *~build123d.build_enums.Align] = (*, *)*)[source]**

Location Context: Hex Array

Creates a context of hexagon array of locations for Part or Sketch. When creating
hex locations for an array of circles, set radius to the radius of the circle
plus one half the spacing between the circles.

**Parameters:**

- 
**radius** (*float*) – distance from origin to vertices (major), or
optionally from the origin to side (minor or apothem)
with major_radius = False

- 
**x_count** (*int*) – number of points ( > 0 )

- 
**y_count** (*int*) – number of points ( > 0 )

- 
**major_radius** (*bool*) – If True the radius is the major radius, else the
radius is the minor radius (also known as inscribed radius).
Defaults to False.

- 
**align** (*Union**[**Align**, **tuple**[**Align**, **Align**]**]**, **optional*) – align min, center, or max of object.
Defaults to (Align.CENTER, Align.CENTER).

**Variables:**

- 
**radius** (*float*) – distance from origin to vertices (major), or
optionally from the origin to side (minor or apothem)
with major_radius = False

- 
**apothem** (*float*) – radius of the inscribed circle, also known as minor radius

- 
**x_count** (*int*) – number of points ( > 0 )

- 
**y_count** (*int*) – number of points ( > 0 )

- 
**major_radius** (*bool*) – If True the radius is the major radius, else the
radius is the minor radius (also known as inscribed radius).

- 
**align** (*Union**[**Align**, **tuple**[**Align**, **Align**]**]*) – align min, center, or max of object.

- 
**diagonal** (*float*) – major radius

- 
**local_locations** (*list{Location}*) – locations relative to workplane

**Raises:**

**ValueError** – Spacing and count must be > 0

**
local_locations**

values independent of workplanes

**
*class *PolarLocations(*radius: float*, *count: int*, *start_angle: float = 0.0*, *angular_range: float = 360.0*, *rotate: bool = True*, *endpoint: bool = False*)[source]**

Location Context: Polar Array

Creates a context of polar array of locations for Part or Sketch

**Parameters:**

- 
**radius** (*float*) – array radius

- 
**count** (*int*) – Number of points to push

- 
**start_angle** (*float**, **optional*) – angle to first point from +ve X axis. Defaults to 0.0.

- 
**angular_range** (*float**, **optional*) – magnitude of array from start angle. Defaults to 360.0.

- 
**rotate** (*bool**, **optional*) – Align locations with arc tangents. Defaults to True.

- 
**endpoint** (*bool**, **optional*) – If True, start_angle + angular_range is the last sample.
Otherwise, it is not included. Defaults to False.

**Variables:**

**local_locations** (*list{Location}*) – locations relative to workplane

**Raises:**

**ValueError** – Count must be greater than or equal to 1

**
local_locations**

values independent of workplanes