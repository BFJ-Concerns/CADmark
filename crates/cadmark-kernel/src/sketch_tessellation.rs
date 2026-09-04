// Sketch extraction — converts the OCP shape behind a script's sketch into
// the kernel-neutral profile the renderer draws: curves as polylines,
// corners as points, enclosed regions as triangulated areas, all placed on
// the plane the sketch was drawn on.
//
// A solid's tessellation (`tessellation.rs`) carries triangles and edges
// only; a sketch needs point geometry and filled regions that the solid
// mesh has no place for, which is why it is extracted separately rather
// than folded into `TessellatedMesh`.

use cadmark_core::sketch::{SketchCorner, SketchCurve, SketchPlane, SketchProfile, SketchRegion};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::tessellation::TessellationError;

/// Python source for sketch extraction. Topology is indexed through
/// `TopExp.MapShapes_s`, so an edge two regions share is one curve with one
/// stable ID rather than one per region — the IDs a later unit needs to
/// resolve a picked curve back to the sketch's own topology.
const SKETCH_SOURCE: &std::ffi::CStr = c"
from OCP.BRep import BRep_Tool
from OCP.BRepAdaptor import BRepAdaptor_Curve, BRepAdaptor_Surface
from OCP.BRepMesh import BRepMesh_IncrementalMesh
from OCP.GCPnts import GCPnts_TangentialDeflection
from OCP.GeomAbs import GeomAbs_SurfaceType
from OCP.TopAbs import TopAbs_EDGE, TopAbs_FACE, TopAbs_REVERSED, TopAbs_VERTEX
from OCP.TopExp import TopExp
from OCP.TopLoc import TopLoc_Location
from OCP.TopoDS import TopoDS
from OCP.TopTools import TopTools_IndexedMapOfShape
import math


def _cadmark_indexed(shape, kind):
    indexed = TopTools_IndexedMapOfShape()
    TopExp.MapShapes_s(shape, kind, indexed)
    return indexed


def _cadmark_normalise(vector):
    length = math.sqrt(sum(component * component for component in vector))
    if length < 1e-12:
        return None
    return [component / length for component in vector]


def _cadmark_perpendicular(normal):
    # Any unit vector in the plane; the axis the normal leans on least
    # keeps the cross product well conditioned.
    axis = min(range(3), key=lambda index: abs(normal[index]))
    seed = [0.0, 0.0, 0.0]
    seed[axis] = 1.0
    cross = [
        normal[1] * seed[2] - normal[2] * seed[1],
        normal[2] * seed[0] - normal[0] * seed[2],
        normal[0] * seed[1] - normal[1] * seed[0],
    ]
    return _cadmark_normalise(cross) or [1.0, 0.0, 0.0]


def _cadmark_sketch_plane(faces, curves):
    # A planar face states the plane exactly, orientation included.
    for index in range(1, faces.Extent() + 1):
        face = TopoDS.Face_s(faces.FindKey(index))
        surface = BRepAdaptor_Surface(face)
        if surface.GetType() != GeomAbs_SurfaceType.GeomAbs_Plane:
            continue
        position = surface.Plane().Position()
        origin = position.Location()
        normal = position.Direction()
        x_axis = position.XDirection()
        normal = [normal.X(), normal.Y(), normal.Z()]
        if face.Orientation() == TopAbs_REVERSED:
            normal = [-component for component in normal]
        return {
            'origin': [origin.X(), origin.Y(), origin.Z()],
            'normal': normal,
            'x_axis': [x_axis.X(), x_axis.Y(), x_axis.Z()],
        }

    # A sketch of curves alone has no face to ask, so the plane is the one
    # its points lie in: Newell's normal over every curve, which is stable
    # for a closed profile and for an open one alike.
    points = [point for curve in curves for point in curve['points']]
    normal = [0.0, 0.0, 0.0]
    for index in range(len(points)):
        current = points[index]
        following = points[(index + 1) % len(points)] if len(points) > 1 else current
        normal[0] += (current[1] - following[1]) * (current[2] + following[2])
        normal[1] += (current[2] - following[2]) * (current[0] + following[0])
        normal[2] += (current[0] - following[0]) * (current[1] + following[1])
    normal = _cadmark_normalise(normal) or [0.0, 0.0, 1.0]
    origin = points[0] if points else [0.0, 0.0, 0.0]
    x_axis = None
    if len(points) > 1:
        x_axis = _cadmark_normalise([points[1][axis] - points[0][axis] for axis in range(3)])
    return {
        'origin': origin,
        'normal': normal,
        'x_axis': x_axis or _cadmark_perpendicular(normal),
    }


def _cadmark_sketch_profile(shape, linear_deflection=0.1, angular_deflection=0.5):
    \"\"\"Extract a sketch's curves, corners, and enclosed regions.\"\"\"
    mesh = BRepMesh_IncrementalMesh(shape, linear_deflection, False, angular_deflection, True)
    mesh.Perform()

    faces = _cadmark_indexed(shape, TopAbs_FACE)
    edges = _cadmark_indexed(shape, TopAbs_EDGE)
    vertices = _cadmark_indexed(shape, TopAbs_VERTEX)

    regions = []
    for index in range(1, faces.Extent() + 1):
        face = TopoDS.Face_s(faces.FindKey(index))
        location = TopLoc_Location()
        triangulation = BRep_Tool.Triangulation_s(face, location)
        if triangulation is None:
            continue
        transform = location.Transformation()
        region_vertices = []
        for node in range(1, triangulation.NbNodes() + 1):
            point = triangulation.Node(node).Transformed(transform)
            region_vertices.append([point.X(), point.Y(), point.Z()])
        region_indices = []
        for triangle in range(1, triangulation.NbTriangles() + 1):
            first, second, third = triangulation.Triangle(triangle).Get()
            region_indices.extend([first - 1, second - 1, third - 1])
        if region_indices:
            regions.append({
                'region_id': index - 1,
                'vertices': region_vertices,
                'indices': region_indices,
            })

    curves = []
    for index in range(1, edges.Extent() + 1):
        edge = TopoDS.Edge_s(edges.FindKey(index))
        try:
            adaptor = BRepAdaptor_Curve(edge)
            deflector = GCPnts_TangentialDeflection(adaptor, angular_deflection, linear_deflection)
            points = []
            for point_index in range(1, deflector.NbPoints() + 1):
                point = deflector.Value(point_index)
                points.append([point.X(), point.Y(), point.Z()])
        except Exception:
            # A degenerate edge carries no drawable curve; the rest of the
            # profile still draws.
            continue
        if len(points) > 1:
            curves.append({'curve_id': index - 1, 'points': points})

    corners = []
    for index in range(1, vertices.Extent() + 1):
        point = BRep_Tool.Pnt_s(TopoDS.Vertex_s(vertices.FindKey(index)))
        corners.append({'corner_id': index - 1, 'position': [point.X(), point.Y(), point.Z()]})

    return {
        'plane': _cadmark_sketch_plane(faces, curves),
        'curves': curves,
        'corners': corners,
        'regions': regions,
    }
";

/// Code to unwrap the sketch object and extract its profile. A builder's
/// placed result (`sketch`, `line`) is preferred, so a profile drawn on a
/// workplane other than XY lands where the script put it; those properties
/// rebuild through OCP copies, which the provenance instrumentation refuses
/// when no user frame is on the stack, so the fallback takes the builder's
/// stored local object and places it on the builder's own workplane
/// directly — a location applied to the shape, no rebuild involved.
const EXTRACT_CODE: &std::ffi::CStr = c"
_cadmark_sketch_object = None
_cadmark_sketch_location = None
for _cadmark_placed in ('sketch', 'line'):
    try:
        _cadmark_candidate = getattr(_cadmark_sketch_result, _cadmark_placed, None)
    except Exception:
        _cadmark_candidate = None
    if _cadmark_candidate is not None and getattr(_cadmark_candidate, 'wrapped', None) is not None:
        _cadmark_sketch_object = _cadmark_candidate
        break
if _cadmark_sketch_object is None:
    _cadmark_sketch_object = getattr(_cadmark_sketch_result, '_obj', None) or _cadmark_sketch_result
    _cadmark_workplanes = getattr(_cadmark_sketch_result, 'workplanes', None) or []
    if _cadmark_workplanes:
        _cadmark_sketch_location = getattr(_cadmark_workplanes[0].location, 'wrapped', None)
_cadmark_sketch_shape = getattr(_cadmark_sketch_object, 'wrapped', _cadmark_sketch_object)
if _cadmark_sketch_shape is None:
    raise TypeError('no OCP shape behind %r' % (type(_cadmark_sketch_object).__name__,))
if _cadmark_sketch_location is not None:
    _cadmark_sketch_shape = _cadmark_sketch_shape.Located(_cadmark_sketch_location)
_cadmark_sketch_output = _cadmark_sketch_profile(_cadmark_sketch_shape)
";

/// Extract the profile of `sketch` — a build123d sketch builder, sketch,
/// face, wire, or edge — as plain data.
pub fn extract_profile(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    sketch: &Bound<'_, PyAny>,
) -> Result<SketchProfile, TessellationError> {
    namespace
        .set_item("_cadmark_sketch_result", sketch)
        .map_err(TessellationError::Python)?;
    py.run(SKETCH_SOURCE, Some(namespace), None)
        .map_err(TessellationError::Python)?;
    py.run(EXTRACT_CODE, Some(namespace), None)
        .map_err(TessellationError::Python)?;

    let output = namespace
        .get_item("_cadmark_sketch_output")?
        .ok_or_else(|| TessellationError::NoShape("Sketch extraction produced no result".into()))?;
    parse_profile(&output)
}

fn parse_profile(output: &Bound<'_, PyAny>) -> Result<SketchProfile, TessellationError> {
    let dict = output.downcast::<PyDict>().map_err(|_| {
        TessellationError::NoShape("Sketch extraction produced a non-dict result".into())
    })?;

    let plane = dict
        .get_item("plane")?
        .ok_or_else(|| TessellationError::NoShape("Sketch extraction produced no plane".into()))?;
    let plane = plane.downcast::<PyDict>().map_err(|_| {
        TessellationError::NoShape("Sketch extraction produced a non-dict plane".into())
    })?;
    let plane = SketchPlane {
        origin: point_field(plane, "origin")?,
        normal: point_field(plane, "normal")?,
        x_axis: point_field(plane, "x_axis")?,
    };

    let mut curves = Vec::new();
    for entry in entries(dict, "curves")? {
        let entry = as_dict(&entry)?;
        curves.push(SketchCurve {
            curve_id: field(&entry, "curve_id")?.extract()?,
            points: points(&field(&entry, "points")?)?,
        });
    }

    let mut corners = Vec::new();
    for entry in entries(dict, "corners")? {
        let entry = as_dict(&entry)?;
        corners.push(SketchCorner {
            corner_id: field(&entry, "corner_id")?.extract()?,
            position: point_field(&entry, "position")?,
        });
    }

    let mut regions = Vec::new();
    for entry in entries(dict, "regions")? {
        let entry = as_dict(&entry)?;
        regions.push(SketchRegion {
            region_id: field(&entry, "region_id")?.extract()?,
            vertices: points(&field(&entry, "vertices")?)?,
            indices: field(&entry, "indices")?.extract()?,
        });
    }

    Ok(SketchProfile {
        plane,
        curves,
        corners,
        regions,
    })
}

fn as_dict<'py>(value: &Bound<'py, PyAny>) -> Result<Bound<'py, PyDict>, TessellationError> {
    value.downcast::<PyDict>().map(Clone::clone).map_err(|_| {
        TessellationError::NoShape("Sketch extraction produced a non-dict entry".into())
    })
}

fn field<'py>(
    dict: &Bound<'py, PyDict>,
    key: &str,
) -> Result<Bound<'py, PyAny>, TessellationError> {
    dict.get_item(key)?
        .ok_or_else(|| TessellationError::NoShape(format!("Sketch extraction produced no {key}")))
}

fn entries<'py>(
    dict: &Bound<'py, PyDict>,
    key: &str,
) -> Result<Vec<Bound<'py, PyAny>>, TessellationError> {
    let value = field(dict, key)?;
    let list = value.downcast::<PyList>().map_err(|_| {
        TessellationError::NoShape(format!("Sketch extraction produced a non-list {key}"))
    })?;
    Ok(list.iter().collect())
}

fn point_field(dict: &Bound<'_, PyDict>, key: &str) -> Result<[f32; 3], TessellationError> {
    let value: [f64; 3] = field(dict, key)?.extract()?;
    Ok([value[0] as f32, value[1] as f32, value[2] as f32])
}

fn points(value: &Bound<'_, PyAny>) -> Result<Vec<[f32; 3]>, TessellationError> {
    let raw: Vec<[f64; 3]> = value.extract()?;
    Ok(raw
        .into_iter()
        .map(|point| [point[0] as f32, point[1] as f32, point[2] as f32])
        .collect())
}
