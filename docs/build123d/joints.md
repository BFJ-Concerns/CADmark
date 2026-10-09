<!-- Source: https://build123d.readthedocs.io/en/latest/joints.html -->

> Adapted from the [build123d documentation](https://build123d.readthedocs.io/en/latest/joints.html), Copyright 2022 Gumyr, under the [Apache License 2.0](LICENSE). Converted to Markdown and edited for CADmark by BFJ Concerns; see [NOTICE](../../NOTICE).

# Joints

## Overview

Joints enable Solid and Compound objects to be positioned relative to each other with motions equivalent to physical joints. Joints always work in pairs, connecting one joint type to another as specified in the compatibility table below.

| Joint Type | Connects To | Example |
|---|---|---|
| BallJoint | RigidJoint | Gimbal |
| CylindricalJoint | RigidJoint | Screw |
| LinearJoint | RigidJoint, RevoluteJoint | Slider or Pin Slot |
| RevoluteJoint | RigidJoint | Hinge |
| RigidJoint | RigidJoint | Fixed |

Each object can have multiple labelled joints. All joint objects include a `symbol` property for visualisation, with built-in support in the ocp-vscode viewer.

**Note:** When joints are created within a `BuildPart` builder context, the `to_part` parameter is optional, as the builder automatically transfers joints to the created part upon exit.

## Rigid Joint

A rigid joint positions two components with no freedom of movement. Instantiation requires a `label`, target part (`to_part`), and `joint_location` defining position and orientation:

```python
RigidJoint(label="outlet", to_part=pipe, joint_location=path.location_at(1))
```

The `connect_to()` method repositions another part relative to the stationary joint:

```python
pipe.joints["outlet"].connect_to(flange_outlet.joints["pipe"])
```

This performs a one-time repositioning without binding parts, though assemblies, boolean operations, or `BuildPart` contexts maintain relative locations.

**Note:** Joint labels must be unique within a part.

### Example: Connecting Flanges to a Pipe

The code demonstrates attaching flanges to pipe ends using `location_at()` method and the negate operator (`-`) to reverse direction without changing position.

## Revolute Joint

A revolute joint allows rotation around an axis, like a hinge. The tutorial provides detailed coverage of this joint type.

Key parameters during instantiation include `axis`, `angle_reference`, and `range` for defining circular motion. The `connect_to()` method accepts an `angle` parameter to adjust relative positioning:

```python
connect_to(other: RigidJoint, *, angle: float = None)
```

## Linear Joint

A linear joint enables movement along a single axis, demonstrated by a sliding latch mechanism.

The joint requires an axis and movement limits. Key implementation points:

- `LinearJoint` defines axis and movement boundaries
- `RigidJoint` specifies single location with orientation
- `connect_to()` specifies position within predefined limits

Position changes adjust component placement; values exceeding limits raise exceptions.

```python
connect_to(other: RevoluteJoint, *, position: float = None, angle: float = None)
connect_to(other: RigidJoint, *, position: float = None)
```

## Cylindrical Joint

A cylindrical joint combines linear and revolute functionality, allowing rotation around and movement along a single axis (like a screw).

The `connect_to()` method includes both `position` and `angle` parameters:

```python
hinge_outer.joints["hole2"].connect_to(m6_joint, position=5 * MM, angle=30)
```

```python
connect_to(other: RigidJoint, *, position: float = None, angle: float = None)
```

## Ball Joint

A ball joint enables rotation around all three axes using a gimbal system with three nested rotations.

Limits defined during instantiation prevent mechanical interference. The `connect_to()` method sets three rotation angles:

```python
connect_to(other: RigidJoint, *, angles: RotationLike = None)
```
