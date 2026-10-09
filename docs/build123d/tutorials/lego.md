<!-- Source: https://build123d.readthedocs.io/en/latest/tutorial_lego.html -->

> Adapted from the [build123d documentation](https://build123d.readthedocs.io/en/latest/tutorial_lego.html), Copyright 2022 Gumyr, under the [Apache License 2.0](../LICENSE). Converted to Markdown and edited for CADmark by BFJ Concerns; see [NOTICE](../../../NOTICE).

# Lego Tutorial

This tutorial provides a step by step guide to creating a script to build a parametric Lego block as shown here:

![lego](assets/lego.svg)

## Step 1: Setup

Before getting to the CAD operations, this Lego script needs to import the build123d environment. There are over 100 Python classes in build123d so we'll just import them all with a `from build123d import *`.

The dimensions of the Lego block follow. A key parameter is `pip_count`, the length of the Lego blocks in pips. This parameter must be at least 2.

```python
# Lego dimensions - all in mm
pip_pitch = 8  # Distance between pips
pip_height = 9.6  # Height of each pip
wall_thickness = 1.5  # Wall thickness
pip_radius = 2.4  # Radius of pips
pip_count = 2  # Number of pips (minimum 2)
```

## Step 2: Part Builder

The Lego block will be created by the `BuildPart` builder as it's a discrete three dimensional part; therefore, we'll instantiate a `BuildPart` with the name `lego`.

```python
with BuildPart() as lego:
    # CAD operations will go here
```

## Step 3: Sketch Builder

Lego blocks have quite a bit of internal structure. To create this structure we'll draw a two dimensional sketch that will later be extruded into a three dimensional object. As this sketch will be part of the lego part, we'll create a sketch builder in the context of the part builder as follows:

```python
with BuildPart() as lego:
    with BuildSketch() as plan:
        # Sketch objects will go here
```

Note that builder instance names are optional - we'll use `plan` to reference the sketch. Also note that all sketch objects are filled or 2D faces not just perimeter lines.

## Step 4: Perimeter Rectangle

The first object in the sketch is going to be a rectangle with the dimensions of the outside of the Lego block. The following step is going to refer to this rectangle, so it will be assigned the identifier `perimeter`.

```python
with BuildPart() as lego:
    with BuildSketch() as plan:
        perimeter = Rectangle(
            width=pip_pitch * pip_count,
            height=pip_pitch,
            align=(Align.CENTER, Align.CENTER),
        )
```

Once the `Rectangle` object is created the sketch appears as follows:

![lego_step4](assets/lego_step4.svg)

## Step 5: Offset to Create Walls

To create the walls of the block the rectangle that we've created needs to be hollowed out. This will be done with the `Offset` operation which is going to create a new object from `perimeter`.

```python
with BuildPart() as lego:
    with BuildSketch() as plan:
        perimeter = Rectangle(
            width=pip_pitch * pip_count,
            height=pip_pitch,
            align=(Align.CENTER, Align.CENTER),
        )
        Offset(
            perimeter,
            amount=-wall_thickness,
            kind=Kind.INTERSECTION,
            mode=Mode.SUBTRACT,
        )
```

The first parameter to `Offset` is the reference object. The `amount` is a negative value to indicate that the offset should be internal. The `kind` parameter controls the shape of the corners - `Kind.INTERSECTION` will create square corners. Finally, the `mode` parameter controls how this object will be placed in the sketch - in this case subtracted from the existing sketch.

The result is shown here:

![lego_step5](assets/lego_step5.svg)

Now the sketch consists of a hollow rectangle.

## Step 6: Create Internal Grid

The interior of the Lego block has small ridges on all four internal walls. These ridges will be created as a grid of thin rectangles so the positions of the centres of these rectangles need to be defined. A pair of `GridLocations` location contexts will define these positions, one for the horizontal bars and one for the vertical bars. As the `Rectangle` objects are in the scope of a location context (`GridLocations` in this case) that defined multiple points, multiple rectangles are created.

```python
with BuildPart() as lego:
    with BuildSketch() as plan:
        perimeter = Rectangle(
            width=pip_pitch * pip_count,
            height=pip_pitch,
            align=(Align.CENTER, Align.CENTER),
        )
        Offset(
            perimeter,
            amount=-wall_thickness,
            kind=Kind.INTERSECTION,
            mode=Mode.SUBTRACT,
        )
        # Horizontal bars
        with GridLocations(
            x_spacing=pip_pitch,
            y_spacing=pip_pitch * 2,
            x_count=1,
            y_count=2,
        ):
            Rectangle(width=pip_pitch - 2 * wall_thickness, height=wall_thickness)
        # Vertical bars
        with GridLocations(
            x_spacing=pip_pitch,
            y_spacing=pip_pitch,
            x_count=pip_count,
            y_count=1,
        ):
            Rectangle(width=wall_thickness, height=pip_pitch - 2 * wall_thickness)
```

Here we can see that the first `GridLocations` creates two positions which causes two horizontal rectangles to be created. The second `GridLocations` works in the same way but creates `pip_count` positions and therefore `pip_count` rectangles.

The result looks like this:

![lego_step6](assets/lego_step6.svg)

## Step 7: Create Ridges

To convert the internal grid to ridges, the centre needs to be removed. This will be done with another `Rectangle`.

```python
with BuildPart() as lego:
    with BuildSketch() as plan:
        perimeter = Rectangle(
            width=pip_pitch * pip_count,
            height=pip_pitch,
            align=(Align.CENTER, Align.CENTER),
        )
        Offset(
            perimeter,
            amount=-wall_thickness,
            kind=Kind.INTERSECTION,
            mode=Mode.SUBTRACT,
        )
        # Horizontal bars
        with GridLocations(
            x_spacing=pip_pitch,
            y_spacing=pip_pitch * 2,
            x_count=1,
            y_count=2,
        ):
            Rectangle(width=pip_pitch - 2 * wall_thickness, height=wall_thickness)
        # Vertical bars
        with GridLocations(
            x_spacing=pip_pitch,
            y_spacing=pip_pitch,
            x_count=pip_count,
            y_count=1,
        ):
            Rectangle(width=wall_thickness, height=pip_pitch - 2 * wall_thickness)
        # Remove centre to create ridges
        Rectangle(
            width=pip_pitch - 4 * wall_thickness,
            height=pip_pitch - 4 * wall_thickness,
            mode=Mode.SUBTRACT,
        )
```

The `Rectangle` is subtracted from the sketch to leave the ridges as follows:

![lego_step7](assets/lego_step7.svg)

## Step 8: Hollow Circles

Lego blocks use a set of internal hollow cylinders that the pips push against to hold two blocks together. These will be created with `Circle`.

```python
with BuildPart() as lego:
    with BuildSketch() as plan:
        # ... previous steps ...
        # Hollow circles
        with GridLocations(
            x_spacing=pip_pitch,
            y_spacing=pip_pitch,
            x_count=pip_count,
            y_count=1,
        ):
            Circle(radius=pip_radius - wall_thickness, mode=Mode.SUBTRACT)
            Circle(radius=pip_radius, mode=Mode.ADD)
```

Here another `GridLocations` is used to position the centres of the circles. Note that since both `Circle` objects are in the scope of the location context, both circles will be positioned at these locations.

Once the circles are added, the sketch is complete and looks as follows:

![lego_step8](assets/lego_step8.svg)

## Step 9: Extruding Sketch into Walls

Now that the sketch is complete it needs to be extruded into the three dimensional wall object.

```python
with BuildPart() as lego:
    with BuildSketch() as plan:
        # ... all the sketch steps ...
    extrude(amount=pip_height)
```

Note how the `extrude` operation is no longer in the `BuildSketch` scope and has returned back into the `BuildPart` scope. This causes `BuildSketch` to exit and transfer the sketch that we've created to `BuildPart` for further processing by `extrude`.

The result is:

![lego_step9](assets/lego_step9.svg)

## Step 10: Adding a Top

Now that the walls are complete, the top of the block needs to be added. Although this could be done with another sketch, we'll add a box to the top of the walls.

```python
with BuildPart() as lego:
    with BuildSketch() as plan:
        # ... all the sketch steps ...
    extrude(amount=pip_height)

    # Add top face
    with Locations(
        lego.vertices()
        .sort_by(Axis.Z)[-1]
        .location.move((0, 0, lego.vertices().sort_by(Axis.Z)[-1].Z))
    ):
        Box(
            length=pip_pitch * pip_count,
            width=pip_pitch,
            height=wall_thickness,
            align=(Align.CENTER, Align.CENTER, Align.MIN),
        )
```

To position the top, we'll describe the top centre of the lego walls with a `Locations` context. To determine the height we'll extract that from the `lego.part` by using the `vertices()` method which returns a list of the positions of all of the vertices of the Lego block so far. Since we're interested in the top, we'll sort by the vertical (Z) axis and take the top of the list `sort_by(Axis.Z)[-1]`. Finally, the `Z` property of this vertex will return just the height of the top.

Within the scope of this `Locations` context, a `Box` is created, centred at the intersection of the x and y axis but not in the z thus aligning with the top of the walls.

The base is closed now as shown here:

![lego_step10](assets/lego_step10.svg)

## Step 11: Adding Pips

The final step is to add the pips to the top of the Lego block. To do this we'll create a new workplane on top of the block where we can position the pips.

```python
with BuildPart() as lego:
    with BuildSketch() as plan:
        # ... all the sketch steps ...
    extrude(amount=pip_height)

    # ... add top face ...

    # Add pips on top
    with BuildSketch(lego.faces().sort_by(Axis.Z)[-1]):
        with GridLocations(
            x_spacing=pip_pitch,
            y_spacing=pip_pitch,
            x_count=pip_count,
            y_count=1,
        ):
            Circle(radius=pip_radius)
    extrude(amount=pip_height)
```

In this case, the workplane is created from the top Face of the Lego block by using the `faces` method and then sorted vertically and taking the top one `sort_by(Axis.Z)[-1]`.

On the new workplane, a grid of locations is created and a number of cylinders are positioned at each location.

![lego](assets/lego.svg)

## Completion

This completes the Lego block. To access the finished product, refer to the builder's internal object as shown here:

| Builder | Object |
|---------|--------|
| BuildLine | line |
| BuildSketch | sketch |
| BuildPart | part |

So in this case the Lego block is `lego.part`. To display the part use `show_object(lego.part)` or `show(lego.part)` depending on the viewer. The part could also be exported to a STL or STEP file by referencing `lego.part`.

> Note: Viewers that don't directly support build123d may require a raw OpenCascade object. In this case, append `.wrapped` to the object (e.g.) `show_object(lego.part.wrapped)`.
