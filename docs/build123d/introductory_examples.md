<!-- Source: https://build123d.readthedocs.io/en/latest/introductory_examples.html -->

> Adapted from the [build123d documentation](https://build123d.readthedocs.io/en/latest/introductory_examples.html), Copyright 2022 Gumyr, under the [Apache License 2.0](LICENSE). Converted to Markdown and edited for CADmark by BFJ Concerns; see [NOTICE](../../NOTICE).

# build123d Introductory Examples

## Overview

This documentation collection demonstrates CAD object creation using build123d, progressing from simple to complex designs. All examples require `from build123d import *` and are intended for builder mode usage in CADmark.

**Key Setup Notes:**
- Use `show(object)` in ocp_vscode or `show_object(object.part)` in CQ-editor for visualization
- Export to STL with `export_stl(object.part, "file.stl")` from a builder context
- Multiple file formats supported including STEP

---

## Example 1: Simple Rectangular Plate

The most basic design element creates a single `Box` primitive.

---

## Example 2: Plate with Hole

Demonstrates Boolean operations using `Mode.SUBTRACT` in a builder context to cut a `Cylinder` from a `Box`.

---

## Example 3: Extruded Prismatic Solid

Introduces 2D sketching with `BuildSketch`, adding a `Circle` with a subtracted `Rectangle`, then using the `extrude()` operation to create a 3D solid.

---

## Example 4: Building Profiles with Lines and Arcs

Complex profiles combine line and arc segments using `BuildSketch`. The `make_face()` operation converts pending line segments into closed faces.

**Key Principle:** "To build a closed face it requires line segments that form a closed shape."

---

## Example 5: Moving the Current Working Point

Objects are positioned using `Locations` within builder contexts.

---

## Example 6: Using Point Lists

Multiple objects are created at various locations simultaneously using point lists and vectorized operations.

---

## Example 7: Polygons

Regular polygon creation with `RegularPolygon`, positioned at multiple locations via loops or list comprehensions.

---

## Example 8: Polylines

`Polyline` creates shapes from chained points connected by lines. This example demonstrates creating an i-beam profile using `Polyline` and `mirror()`.

---

## Example 9: Selectors, Fillets, and Chamfers

Introduces edge selection and modification:
- `fillet()` rounds edges
- `chamfer()` bevels edges
- `edges()` selects all edges
- `group_by(Axis.Z)` groups edges by position

---

## Example 10: Select Last and Hole

Demonstrates selecting recently modified edges using `Select.LAST` in builder mode. Introduces `Hole` for automatic through-hole cutting.

---

## Example 11: Face-Based Workplanes and GridLocations

Uses faces as planes for `BuildSketch` placement. `GridLocations` creates grids of points for simultaneous multi-object placement. Negative extrude amounts with `Mode.SUBTRACT` cut features.

**Important:** "Note that the face used as input to BuildSketch needs to be Planar or unpredictable behavior can result."

---

## Example 12: Spline-Defined Edges

`Spline` curves define complex edge profiles through point collections.

---

## Example 13: Fastener Holes and PolarLocations

Counter-sink and counter-bore holes for recessed fasteners. `PolarLocations` creates radially distributed point patterns.

---

## Example 14: Line Position Operators and Sweep

- `position_at()` (`@`) finds normalized positions along lines (0–1)
- `tangent_at()` (`%`) returns line direction at given points
- `sweep()` extrudes pending faces along a path

---

## Example 15: Mirroring Symmetric Geometry

`mirror()` creates symmetric shapes with fewer commands. The `@` operator extracts position vectors; accessing components via `.Y` extracts specific dimensions.

---

## Example 16: Mirroring 3D Objects

Mirrors `BuildPart` objects using `Plane.offset()` to shift the mirror plane along the normal direction.

---

## Example 17: Mirroring From Faces

Converts selected faces into `Plane` objects for mirroring operations.

---

## Example 18: Workplanes on Faces

Selects the top face, draws a rectangle, and uses negative extrude with `Mode.SUBTRACT` to cut inward.

---

## Example 19: Vertex-Located Workplanes

Two vertex selection strategies:
1. `group_by(Axis.X)` for specific vertex selection
2. Custom axis with `sort_by()` for directional selection

Extract X/Y positions to avoid Z-offset from workplane.

---

## Example 20: Offset Sketch Workplane

Planes are positioned coincident with faces and then offset from the original position.

---

## Example 21: Centered Workplanes

Uses origin and z-direction of existing parts to position new features perpendicular and offset.

---

## Example 22: Rotated Workplanes

Uses `Plane.rotated()` (builder) or multiplication operator `*` (algebra) to create rotated workplanes. `GridLocations` places features on rotated planes.

---

## Example 23: Revolve

Creates rotational geometry from sketches. Critical requirement: "Absolutely critical that the sketch is only on one side of the axis of rotation before Revolve is called." Use `split()` with `Plane.ZY` to isolate one side.

---

## Example 24: Loft

Joins dissimilar shapes (e.g., circle to rectangle) creating conical-like geometry. `loft()` automatically uses pending faces from multiple `BuildSketches`. Works best with parallel input faces.

---

## Example 25: Offset Sketch (2D)

2D `offset()` transforms sketch faces inward or outward with corner extension options via the `Kind` parameter.

---

## Example 26: Offset Part (3D Shell)

3D `offset()` creates thin-walled structures with minimal operations. The `openings` parameter removes selected faces. Note: Self-intersecting edges/faces can break offsets.

---

## Example 27: Splitting Objects

`split()` divides objects using planes, retaining either or both halves.

---

## Example 28: Face-Based Feature Locations

Creates helper shapes (often with `Mode.PRIVATE`) using faces to position holes or other features in target objects.

---

## Example 29: Classic OCC Bottle

The OpenCascade reference design, recreated using 3D offset with `openings` parameter to create bottle openings.

---

## Example 30: Bezier Curves

`Polyline` and `Bezier` accept point lists; Bezier additionally uses weights (`wts`) for control. Closed curves convert to faces and extrude.

---

## Example 31: Nesting Locations

Nested location contexts create hierarchical shape groups. `PolarLocations` rotates child groups by default.

---

## Example 32: Python For-Loop Pattern

Standard loops iterate over sketch faces, progressively modifying extrusion amounts. Use `Mode.PRIVATE` in `BuildSketch` to defer face addition until loop processing.

---

## Example 33: Function-Based Design

Python functions return sketch objects parameterized by inputs, enabling progressive shape modification in loops.

---

## Example 34: Embossed/Debossed Text

Text placement uses `Align` for positioning control. Reusing face variables (`topf`) ensures correct positioning on original surfaces rather than newly modified geometry.

---

## Example 35: Slots

Combines `SlotCenterToCenter` with `BuildLine` and `RadiusArc` to create `SlotArc` instances.

---

## Example 36: Extrude Until

`extrude()` with `Until.NEXT` or `Until.LAST` extends to non-planar faces without calculating exact distances.

---

## Key Concepts Summary

| Concept | Description |
|---------|-------------|
| **Modes** | SUBTRACT, PRIVATE control operation behaviour |
| **Operators** | `+` (ADD), `-` (SUBTRACT), `*` (position/rotate), `@` (position_at), `%` (tangent_at) |
| **Selectors** | `edges()`, `faces()`, `vertices()`, `group_by()`, `sort_by()` |
| **Locations** | `Locations`, `GridLocations`, `PolarLocations` for multi-object placement |
| **2D Operations** | offset, make_face, split |
| **3D Operations** | extrude, revolve, loft, sweep, fillet, chamfer |
