<!-- Source: https://build123d.readthedocs.io/en/latest/import_export.html -->

# Import/Export Documentation

## Overview

Methods for exporting and importing build123d objects enable seamless data exchange. Basic usage:

```python
with BuildPart() as box_builder:
    Box(1, 1, 1)
export_step(box_builder.part, "box.step")
```

## File Formats

### 3MF

The 3MF (3D Manufacturing Format) file format is a versatile and modern standard for representing 3D models used in additive manufacturing, 3D printing, and other applications. It supports advanced features including colour, texture mapping, and multi-material definitions.

### BREP

Boundary Representation files store 3D geometry using topological entities like vertices, edges, and faces. Widely supported across CAD software for precise model representation.

### DXF

The DXF (Drawing Exchange Format) file format is a widely used standard for representing 2D and 3D drawings, primarily used in computer-aided design (CAD) applications.

### glTF

Developed by the Khronos Group, glTF serves as a royalty-free specification for the efficient transmission and loading of 3D models. Often called the "JPEG of 3D" for its versatility.

### STL

The STL (STereoLithography) file format is a widely used file format in 3D printing that represents geometry using triangular facets, supporting both geometry and colour information.

### STEP

The STEP (Standard for the Exchange of Product model data) file format is a widely used standard (ISO 10303) enabling seamless interchange across CAD/CAM systems.

### SVG

The SVG (Scalable Vector Graphics) file format is an XML-based standard used for describing 2D vector graphics, ideal for web applications and resolution-independent graphics.

## 2D Exporters

ExportDXF and ExportSVG classes manage document creation, layer addition, and shape insertion before writing files. The workflow involves:

1. Creating a document with specific properties
2. Adding layers and shapes
3. Writing to file

### 3D to 2D Projection

Two approaches convert 3D parts to 2D drawings:

- **Section**: Use the `section` operation for cross-sections
- **Projection**: Apply `project_to_viewport()` method, functioning like a camera with configurable origin, orientation, and focus point

Example SVG export with projection:

```python
view_port_origin=(-100, -50, 30)
visible, hidden = part.project_to_viewport(view_port_origin)
max_dimension = max(*Compound(children=visible + hidden).bounding_box().size)
exporter = ExportSVG(scale=100 / max_dimension)
exporter.add_layer("Visible")
exporter.add_layer("Hidden", line_color=(99, 99, 99), line_type=LineType.ISO_DOT)
exporter.add_shape(visible, layer="Visible")
exporter.add_shape(hidden, layer="Hidden")
exporter.write("part_projection.svg")
```

### LineType

Standards define line conventions:
- **ANSI/ASME Y14.2**: U.S. engineering drawing standards
- **ISO 128**: General technical drawing principles
- **ISO 13567**: CAD layer organisation guidelines

The `LineType` Enum provides standardised line types for technical drawings.

## 3D Exporters

- `export_brep()`: Boundary Representation format
- `export_gltf()`: GL Transmission Format
- `export_step()`: STEP format
- `export_stl()`: STL format

## 3D Mesh Export

The `Mesher` class handles 3MF and STL export with rich feature support:

```python
blue_shape = Solid.make_cone(20, 0, 50)
blue_shape.color = Color("blue")
blue_shape.label = "blue"

exporter = Mesher()
exporter.add_shape(blue_shape, part_number="blue-1234-5")
exporter.add_meta_data(name_space="custom", name="test_meta_data", value="hello world")
exporter.write("example.3mf")
```

**Tip**: Use `pack()` function to align multiple components for 3D printing on the same plane.

## 2D Importers

- `import_svg()`: Standard SVG import
- `import_svg_as_buildline_code()`: Convert SVG to build123d code

## 3D Importers

- `import_brep()`: BREP format
- `import_step()`: STEP format
- `import_stl()`: STL format

## 3D Mesh Import

The `Mesher` class reads 3MF and STL files with metadata preservation:

```python
importer = Mesher()
cone, cyl = importer.read("example.3mf")
print(f"Imported model unit: {importer.model_unit}")
print(f"{cone.label=}")
print(f"{cone.color.to_tuple()=}")
```

Returns mesh count, vertex counts, triangle counts, unit information, labels, and colour data.
