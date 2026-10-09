<!-- Source: https://build123d.readthedocs.io/en/latest/moving_objects.html -->

> Adapted from the [build123d documentation](https://build123d.readthedocs.io/en/latest/moving_objects.html), Copyright 2022 Gumyr, under the [Apache License 2.0](LICENSE). Converted to Markdown and edited for CADmark by BFJ Concerns; see [NOTICE](../../NOTICE).

# Moving Objects

In CADmark, object movement is described using builder-mode placement tools and
direct manipulation methods on shapes.

## Builder Mode

In builder mode, object locations are defined before the objects themselves are created. This approach ensures that objects are positioned correctly during the construction process. The following tools are commonly used to specify locations:

1. `Locations` - Use this to define a specific location for the objects within the `with` block.
2. `GridLocations` - Arrange objects in a grid pattern.
3. `PolarLocations` - Position objects in a circular pattern.
4. `HexLocations` - Arrange objects in a hexagonal grid.

> **Note:** The location(s) of an object must be defined prior to its creation when using builder mode.

### Example:

```python
with Locations((10, 20, 30)):
    Box(5, 5, 5)
```

## Direct Manipulation Methods

The following methods allow for direct manipulation of a shape's location and orientation after it has been created. These methods offer a mix of absolute and relative transformations.

### Position

**Absolute Position:** Set the position directly.

```python
shape.position = (x, y, z)
```

**Relative Position:** Adjust the position incrementally.

```python
shape.position += (x, y, z)
shape.position -= (x, y, z)
```

### Orientation

**Absolute Orientation:** Set the orientation directly.

```python
shape.orientation = (X, Y, Z)
```

**Relative Orientation:** Adjust the orientation incrementally.

```python
shape.orientation += (X, Y, Z)
shape.orientation -= (X, Y, Z)
```

### Movement Methods

**Relative Move:**

```python
shape.move(Location)
```

**Relative Move of Copy:**

```python
relocated_shape = shape.moved(Location)
```

**Absolute Move:**

```python
shape.locate(Location)
```

**Absolute Move of Copy:**

```python
relocated_shape = shape.located(Location)
```

### Transformation (Translation and Rotation)

> **Note:** These methods have an optional `transform` parameter which allows the user to transform the base object itself which is quite slow and potentially problematic as opposed to just changing the object's internal `Location`.

**Translation:** Move a shape relative to its current position.

```python
relocated_shape = shape.translate((x, y, z))
```

**Rotation:** Rotate a shape around a specified axis by a given angle.

```python
rotated_shape = shape.rotate(Axis, angle_in_degrees)
```
