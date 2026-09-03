// Geometric measurement of the executed model — per-element descriptors for
// the AI's identification context and whole-model figures for the status bar
// and the edit regression check.

use cadmark_core::geometry::{
    EdgeDescriptor, FaceDescriptor, GeometryDescriptors, ModelSummary, VertexDescriptor,
};
use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Measures every face, edge and vertex in the final topology maps, in the
/// same order the provenance ledger and picking IDs use.
const MEASUREMENT_SOURCE: &std::ffi::CStr = c"
from OCP.Bnd import Bnd_Box
from OCP.BRep import BRep_Tool
from OCP.BRepAdaptor import BRepAdaptor_Curve, BRepAdaptor_Surface
from OCP.BRepBndLib import BRepBndLib
from OCP.BRepGProp import BRepGProp, BRepGProp_Face
from OCP.BRepTools import BRepTools
from OCP.GeomAbs import GeomAbs_CurveType, GeomAbs_SurfaceType
from OCP.gp import gp_Pnt, gp_Vec
from OCP.GProp import GProp_GProps
from OCP.TopoDS import TopoDS

_surface_names = {
    GeomAbs_SurfaceType.GeomAbs_Plane: 'plane',
    GeomAbs_SurfaceType.GeomAbs_Cylinder: 'cylinder',
    GeomAbs_SurfaceType.GeomAbs_Cone: 'cone',
    GeomAbs_SurfaceType.GeomAbs_Sphere: 'sphere',
    GeomAbs_SurfaceType.GeomAbs_Torus: 'torus',
    GeomAbs_SurfaceType.GeomAbs_BezierSurface: 'bezier',
    GeomAbs_SurfaceType.GeomAbs_BSplineSurface: 'bspline',
    GeomAbs_SurfaceType.GeomAbs_SurfaceOfRevolution: 'revolution',
    GeomAbs_SurfaceType.GeomAbs_SurfaceOfExtrusion: 'extrusion',
    GeomAbs_SurfaceType.GeomAbs_OffsetSurface: 'offset',
}
_curve_names = {
    GeomAbs_CurveType.GeomAbs_Line: 'line',
    GeomAbs_CurveType.GeomAbs_Circle: 'circle',
    GeomAbs_CurveType.GeomAbs_Ellipse: 'ellipse',
    GeomAbs_CurveType.GeomAbs_Hyperbola: 'hyperbola',
    GeomAbs_CurveType.GeomAbs_Parabola: 'parabola',
    GeomAbs_CurveType.GeomAbs_BezierCurve: 'bezier',
    GeomAbs_CurveType.GeomAbs_BSplineCurve: 'bspline',
    GeomAbs_CurveType.GeomAbs_OffsetCurve: 'offset',
}


def _point(p):
    return [p.X(), p.Y(), p.Z()]


def measure_faces(shapes):
    faces = []
    for face in shapes:
        face = TopoDS.Face_s(face)
        props = GProp_GProps()
        BRepGProp.SurfaceProperties_s(face, props)
        u_min, u_max, v_min, v_max = BRepTools.UVBounds_s(face)
        point = gp_Pnt()
        normal = gp_Vec()
        BRepGProp_Face(face).Normal((u_min + u_max) / 2, (v_min + v_max) / 2, point, normal)
        if normal.Magnitude() > 0:
            normal.Normalize()
        faces.append({
            'surface_type': _surface_names.get(BRepAdaptor_Surface(face).GetType(), 'other'),
            'area': props.Mass(),
            'centre': _point(props.CentreOfMass()),
            'normal': _point(normal),
        })
    return faces


def measure_edges(shapes):
    edges = []
    for edge in shapes:
        edge = TopoDS.Edge_s(edge)
        props = GProp_GProps()
        BRepGProp.LinearProperties_s(edge, props)
        try:
            curve_type = _curve_names.get(BRepAdaptor_Curve(edge).GetType(), 'other')
        except Exception:
            curve_type = 'degenerate'
        edges.append({
            'curve_type': curve_type,
            'length': props.Mass(),
            'centre': _point(props.CentreOfMass()),
        })
    return edges


def measure_vertices(shapes):
    return [{'position': _point(BRep_Tool.Pnt_s(TopoDS.Vertex_s(v)))} for v in shapes]


def measure_model(shape, face_count, edge_count, vertex_count):
    props = GProp_GProps()
    BRepGProp.VolumeProperties_s(shape, props)
    # Measured from the exact surfaces: the triangulation route pads the box
    # by the mesh tolerance, which shows up as a 60.05 mm plate.
    box = Bnd_Box()
    BRepBndLib.AddOptimal_s(shape, box, False, False)
    x_min, y_min, z_min, x_max, y_max, z_max = box.Get()
    return {
        'volume': props.Mass(),
        'bounds_min': [x_min, y_min, z_min],
        'bounds_max': [x_max, y_max, z_max],
        'face_count': face_count,
        'edge_count': edge_count,
        'vertex_count': vertex_count,
    }


def measure(shape, session):
    faces = session.map_values(session.final_maps['face'])
    edges = session.map_values(session.final_maps['edge'])
    vertices = session.map_values(session.final_maps['vertex'])
    return {
        'faces': measure_faces(faces),
        'edges': measure_edges(edges),
        'vertices': measure_vertices(vertices),
        'summary': measure_model(shape, len(faces), len(edges), len(vertices)),
    }
";

/// Measure the finalised model. `session` is the provenance session whose
/// final topology maps define element order.
pub(crate) fn measure(
    py: Python<'_>,
    shape: &Bound<'_, PyAny>,
    session: &Bound<'_, PyAny>,
) -> PyResult<(GeometryDescriptors, ModelSummary)> {
    let namespace = PyDict::new(py);
    py.run(MEASUREMENT_SOURCE, Some(&namespace), None)?;
    let measure = namespace
        .get_item("measure")?
        .expect("measurement source defines measure()");
    let result = measure.call1((shape, session))?;

    let mut faces = Vec::new();
    for face in result.get_item("faces")?.try_iter()? {
        let face = face?;
        faces.push(FaceDescriptor {
            surface_type: face.get_item("surface_type")?.extract()?,
            area: face.get_item("area")?.extract()?,
            centre: face.get_item("centre")?.extract()?,
            normal: face.get_item("normal")?.extract()?,
        });
    }
    let mut edges = Vec::new();
    for edge in result.get_item("edges")?.try_iter()? {
        let edge = edge?;
        edges.push(EdgeDescriptor {
            curve_type: edge.get_item("curve_type")?.extract()?,
            length: edge.get_item("length")?.extract()?,
            centre: edge.get_item("centre")?.extract()?,
        });
    }
    let mut vertices = Vec::new();
    for vertex in result.get_item("vertices")?.try_iter()? {
        let vertex = vertex?;
        vertices.push(VertexDescriptor {
            position: vertex.get_item("position")?.extract()?,
        });
    }
    let summary = result.get_item("summary")?;
    let summary = ModelSummary {
        volume: summary.get_item("volume")?.extract()?,
        bounds_min: summary.get_item("bounds_min")?.extract()?,
        bounds_max: summary.get_item("bounds_max")?.extract()?,
        face_count: summary.get_item("face_count")?.extract()?,
        edge_count: summary.get_item("edge_count")?.extract()?,
        vertex_count: summary.get_item("vertex_count")?.extract()?,
    };

    Ok((
        GeometryDescriptors {
            faces,
            edges,
            vertices,
        },
        summary,
    ))
}
