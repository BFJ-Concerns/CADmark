<!-- Source: https://build123d.readthedocs.io/en/latest/objects.html -->

> Adapted from the [build123d documentation](https://build123d.readthedocs.io/en/latest/objects.html), Copyright 2022 Gumyr, under the [Apache License 2.0](LICENSE). Converted to Markdown and edited for CADmark by BFJ Concerns; see [NOTICE](../../NOTICE).

# Objects

Objects are Python classes that take parameters as inputs and create 1D, 2D or
3D Shapes. For example, a Torus is defined by a major and minor radii. In
CADmark, objects are positioned with builder-mode tools such as `Locations`.

Builder mode example:
```build123d
with BuildPart() as disk:
    with BuildSketch():
        Circle(a)
        with Locations((b, 0.0)):
            Rectangle(c, c, mode=Mode.SUBTRACT)
        with Locations((0, b)):
            Circle(d, mode=Mode.SUBTRACT)
    extrude(amount=c)
```

## Align

2D/Sketch and 3D/Part objects can be aligned relative to themselves, either centred, or justified right or left of each Axis.

For example:

```build123d
with BuildSketch():
    Circle(1, align=(Align.MIN, Align.MIN))
```

This creates a circle whose minimal X and Y values are on the X and Y axis and is located in the top right corner. The `Align` enum has values: `MIN`, `CENTER` and `MAX`.

In 3D the `align` parameter also contains a Z align value but otherwise works in the same way.

Note that the `align` will also accept a single `Align` value which will be used on all axes:

```build123d
with BuildSketch():
    Circle(1, align=Align.MIN)
```

## Mode

With the Builder API the `mode` parameter controls how objects are combined with lines, sketches, or parts under construction. The `Mode` enum has values:

- `ADD`: fuse this object to the object under construction
- `SUBTRACT`: cut this object from the object under construction
- `INTERSECT`: intersect this object with the object under construction
- `REPLACE`: replace the object under construction with this object
- `PRIVATE`: don't interact with the object under construction at all

## 1D Objects

The following objects all can be used in BuildLine contexts. Note that 1D objects are not affected by `Locations` in Builder mode.

### 1D Object Types

- **Airfoil** - Airfoil described by 4 digit NACA profile
- **Bezier** - Curve defined by control points and weights
- **BlendCurve** - Curve blending curvature of two curves
- **CenterArc** - Arc defined by centre, radius, and angles
- **ConstrainedArcs** - Arc(s) constrained by other geometric objects
- **ConstrainedLines** - Line(s) constrained by other geometric objects
- **DoubleTangentArc** - Arc defined by point/tangent pair and other curve
- **EllipticalCenterArc** - Elliptical arc defined by centre, radii and angles
- **EllipticalStartArc** - Elliptical arc defined by start, tangent, radii and angles
- **ParabolicCenterArc** - Parabolic arc defined by vertex, focal length and angles
- **HyperbolicCenterArc** - Hyperbolic arc defined by centre, radii and angles
- **FilletPolyline** - Polyline with filleted corners defined by points and radius
- **Helix** - Helix defined pitch, radius and height
- **IntersectingLine** - Intersecting line defined by start, direction and other line
- **JernArc** - Arc defined by start point, tangent, radius and angle
- **Line** - Line defined by end points
- **PolarLine** - Line defined by start, angle and length
- **Polyline** - Multiple line segments defined by points
- **RadiusArc** - Arc defined by two points and a radius
- **SagittaArc** - Arc defined by two points and a sagitta
- **Spline** - Curve defined by points
- **TangentArc** - Arc defined by two points and a tangent
- **ThreePointArc** - Arc defined by three points
- **ArcArcTangentLine** - Line tangent defined by two arcs
- **ArcArcTangentArc** - Arc tangent defined by two arcs
- **PointArcTangentLine** - Line tangent defined by a point and arc
- **PointArcTangentArc** - Arc tangent defined by a point, direction, and arc

## 2D Objects

### 2D Object Types

- **Arrow** - Arrow with head and path for shaft
- **ArrowHead** - Arrow head with multiple types
- **Circle** - Circle defined by radius
- **DimensionLine** - Dimension line
- **Ellipse** - Ellipse defined by major and minor radius
- **ExtensionLine** - Extension lines for distance or angles
- **Polygon** - Polygon defined by points
- **Rectangle** - Rectangle defined by width and height
- **RectangleRounded** - Rectangle with rounded corners defined by width, height, and radius
- **RegularPolygon** - Regular polygon defined by radius and number of sides
- **SlotArc** - Slot arc defined by arc and height
- **SlotCenterPoint** - Slot defined by two points and a height
- **SlotCenterToCenter** - Slot defined by centre separation and height
- **SlotOverall** - Slot defined by end-to-end length and height
- **TechnicalDrawing** - A technical drawing with descriptions
- **Text** - Text defined by string and font parameters
- **Trapezoid** - Trapezoid defined by width, height and interior angles
- **Triangle** - Triangle defined by one side and two other sides or interior angles

## 3D Objects

### 3D Object Types

- **Box** - Box defined by length, width, height
- **Cone** - Cone defined by radii and height
- **ConvexPolyhedron** - Convex polyhedron defined by points
- **CounterBoreHole** - Counter bore hole defined by radii and depths
- **CounterSinkHole** - Counter sink hole defined by radii, depth and angle
- **Cylinder** - Cylinder defined by radius and height
- **Hole** - Hole defined by radius and depth
- **Sphere** - Sphere defined by radius and arc angles
- **Torus** - Torus defined by major and minor radii
- **Wedge** - Wedge defined by lengths along multiple axes

## Custom Objects

Users can create custom objects by subclassing `BaseLineObject`, `BaseSketchObject`, or `BasePartObject`. Custom classes should include a `mode` parameter to support integration with builders.

Here is an example of a custom sketch object:

```python
class Club(BaseSketchObject):
    def __init__(self, height, rotation, align, mode):
        # Create the sketch of the club suit
        # ... implementation details ...

        # Call parent class initialisation
        super().__init__(shape, align, mode)
```

The custom object can now be used anywhere the built-in objects would be used
within the Builder API.

Key points for custom objects:
- The `__init__` method should contain all parameters used to instantiate the object
- Must always contain a `mode` parameter for builder integration
- Call the parent class's `__init__` method with the created shape and parameters
- Can be used anywhere the corresponding built-in object would be used
