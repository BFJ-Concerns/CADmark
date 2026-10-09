<!-- Source: https://build123d.readthedocs.io/en/latest/assemblies.html -->

> Adapted from the [build123d documentation](https://build123d.readthedocs.io/en/latest/assemblies.html), Copyright 2022 Gumyr, under the [Apache License 2.0](LICENSE). Converted to Markdown and edited for CADmark by BFJ Concerns; see [NOTICE](../../NOTICE).

# Assemblies in build123d

## Overview

Most CAD designs comprise multiple parts arranged in an assembly. In build123d, parts are organised using the `Compound` object, which allows treating them as a single unit for operations like moving or exporting.

## Creating Assemblies

### Assigning Labels

You can track objects by assigning string labels to `Shape` objects. These labels have no uniqueness requirement within the assembly.

### Building the Assembly Structure

Create a `Compound` object and define its hierarchy through `parent` and `children` attributes:

```python
assembly = Compound(
    label="assembly",
    children=[box, lid, inner_hinge, outer_hinge]
)
```

Use the `show_topology()` method to visualise the assembly structure.

### Adding Components

Components can be added by modifying either the `children` attribute of the parent or the `parent` attribute of a child object.

## Shallow vs. Deep Copies

**Deep copies** create complete independent duplicates suitable when modifications are planned.

**Shallow copies** reference the original CAD object whilst copying all metadata. This approach is ideal for assemblies containing multiple identical instances positioned differently, offering substantial performance and storage benefits—"just over 1% of the size" compared to deep copies in typical scenarios.

## Anytree Node Integration

build123d shapes inherit from anytree's `NodeMixin`, providing tree navigation properties:

- `parent`, `children`, `path`
- `ancestors`, `descendants`, `root`
- `siblings`, `leaves`
- `is_leaf`, `is_root`, `height`, `depth`

## Iterating Over Compounds

Use `.solids()` to iterate through all solids in nested assemblies without recursive loops, enabling efficient aggregate calculations like total volume.

## The pack() Function

The `pack()` function arranges objects in a compact 2D layout using bin-packing algorithms to minimise overlap and spacing. The `align_z` parameter is particularly useful for 3D printing, aligning object bottoms to the same plane for streamlined slicing software integration.
