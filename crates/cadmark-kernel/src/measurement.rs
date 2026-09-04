// Geometric measurement of the executed model — per-element descriptors for
// the AI's identification context and whole-model figures for the status bar
// and the edit regression check.

use cadmark_core::geometry::{
    EdgeDescriptor, FaceDescriptor, GeometryDescriptors, MinimumDistance, ModelSummary,
    SolidValidity, TopologyElement, VertexDescriptor,
};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use thiserror::Error;

use crate::protocol::ModelFile;

/// Measures every face, edge and vertex in the final topology maps, in the
/// same order the provenance ledger and picking IDs use.
const MEASUREMENT_SOURCE: &std::ffi::CStr = c"
from OCP.Bnd import Bnd_Box
from OCP.BRep import BRep_Tool
from OCP.BRepAdaptor import BRepAdaptor_Curve, BRepAdaptor_Surface
from OCP.BRepBndLib import BRepBndLib
from OCP.BRepCheck import BRepCheck_Analyzer
from OCP.BRepExtrema import BRepExtrema_DistShapeShape
from OCP.BRepGProp import BRepGProp, BRepGProp_Face
from OCP.BRepTools import BRepTools
from OCP.GeomAbs import GeomAbs_CurveType, GeomAbs_SurfaceType
from OCP.gp import gp_Pnt, gp_Vec
from OCP.GProp import GProp_GProps
from OCP.TopAbs import TopAbs_EDGE, TopAbs_FACE, TopAbs_SHELL, TopAbs_SOLID, TopAbs_VERTEX
from OCP.TopExp import TopExp, TopExp_Explorer
from OCP.TopTools import TopTools_IndexedMapOfShape
from OCP.TopoDS import TopoDS
from build123d import import_brep

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
            curve = BRepAdaptor_Curve(edge)
            curve_type = _curve_names.get(curve.GetType(), 'other')
            radius = curve.Circle().Radius() if curve_type == 'circle' else None
        except Exception:
            curve_type = 'degenerate'
            radius = None
        edges.append({
            'curve_type': curve_type,
            'length': props.Mass(),
            'radius': radius,
            'centre': _point(props.CentreOfMass()),
        })
    return edges


def measure_vertices(shapes):
    return [{'position': _point(BRep_Tool.Pnt_s(TopoDS.Vertex_s(v)))} for v in shapes]


def _children(shape, kind):
    return list(_explore(shape, kind))


def _append_once(neighbours, neighbour):
    if neighbour not in neighbours:
        neighbours.append(neighbour)


def measure_neighbours(session, faces, edges, vertices):
    # Adjacency is read from final OCCT topology.  IDs come from the same
    # final maps as picking and provenance, so this never relies on geometry
    # matching or a kernel type crossing the worker boundary.
    face_neighbours = [[] for _ in faces]
    edge_faces = [[] for _ in edges]
    edge_vertices = [[] for _ in edges]
    vertex_edges = [[] for _ in vertices]

    for face_index, face in enumerate(faces):
        for edge in _children(face, TopAbs_EDGE):
            edge_index = session.topology_index(edge, 'edge')
            _append_once(face_neighbours[face_index], {'kind': 'edge', 'id': edge_index})
            _append_once(edge_faces[edge_index], {'kind': 'face', 'id': face_index})

    for edge_index, edge in enumerate(edges):
        for vertex in _children(edge, TopAbs_VERTEX):
            vertex_index = session.topology_index(vertex, 'vertex')
            _append_once(edge_vertices[edge_index], {'kind': 'vertex', 'id': vertex_index})
            _append_once(vertex_edges[vertex_index], {'kind': 'edge', 'id': edge_index})

    edge_neighbours = [edge_faces[index] + edge_vertices[index] for index in range(len(edges))]
    return face_neighbours, edge_neighbours, vertex_edges


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


def _explore(shape, kind):
    explorer = TopExp_Explorer(shape, kind)
    while explorer.More():
        yield explorer.Current()
        explorer.Next()


def solid_validity(shape):
    # One entry per solid: whether every shell is closed and the OCCT
    # analyser finds no defect. A model with no solid yields nothing.
    results = []
    for solid in _explore(shape, TopAbs_SOLID):
        closed = all(
            BRep_Tool.IsClosed_s(TopoDS.Shell_s(shell))
            for shell in _explore(solid, TopAbs_SHELL)
        )
        analyzer = BRepCheck_Analyzer(solid)
        analyzer.SetParallel(True)
        results.append({'closed': closed, 'valid': analyzer.IsValid()})
    return results


def minimum_distance(model_path, first_kind, first_index, second_kind, second_index):
    shape = import_brep(model_path).wrapped
    kinds = {'face': TopAbs_FACE, 'edge': TopAbs_EDGE, 'vertex': TopAbs_VERTEX}

    def element(kind, index):
        indexed = TopTools_IndexedMapOfShape()
        TopExp.MapShapes_s(shape, kinds[kind], indexed)
        if index < 0 or index >= indexed.Extent():
            raise IndexError(f'{kind} {index} is not in the retained model')
        return indexed.FindKey(index + 1)

    extrema = BRepExtrema_DistShapeShape(
        element(first_kind, first_index), element(second_kind, second_index)
    )
    extrema.Perform()
    if not extrema.IsDone():
        raise RuntimeError('OCCT did not complete the minimum-distance calculation')
    return extrema.Value()


def measure(shape, session):
    faces = session.map_values(session.final_maps['face'])
    edges = session.map_values(session.final_maps['edge'])
    vertices = session.map_values(session.final_maps['vertex'])
    face_neighbours, edge_neighbours, vertex_neighbours = measure_neighbours(session, faces, edges, vertices)
    measured_faces = measure_faces(faces)
    measured_edges = measure_edges(edges)
    measured_vertices = measure_vertices(vertices)
    for measured, neighbours in zip(measured_faces, face_neighbours):
        measured['neighbours'] = neighbours
    for measured, neighbours in zip(measured_edges, edge_neighbours):
        measured['neighbours'] = neighbours
    for measured, neighbours in zip(measured_vertices, vertex_neighbours):
        measured['neighbours'] = neighbours
    return {
        'faces': measured_faces,
        'edges': measured_edges,
        'vertices': measured_vertices,
        'summary': measure_model(shape, len(faces), len(edges), len(vertices)),
    }
";

#[derive(Debug, Error)]
pub enum MeasurementError {
    #[error("minimum-distance measurement failed: {0}")]
    Python(#[from] PyErr),
    #[error("{0} is a whole part; measure between faces, edges or vertices")]
    NotMeasurable(String),
}

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
            neighbours: extract_neighbours(&face.get_item("neighbours")?)?,
        });
    }
    let mut edges = Vec::new();
    for edge in result.get_item("edges")?.try_iter()? {
        let edge = edge?;
        edges.push(EdgeDescriptor {
            curve_type: edge.get_item("curve_type")?.extract()?,
            length: edge.get_item("length")?.extract()?,
            radius: edge.get_item("radius")?.extract()?,
            centre: edge.get_item("centre")?.extract()?,
            neighbours: extract_neighbours(&edge.get_item("neighbours")?)?,
        });
    }
    let mut vertices = Vec::new();
    for vertex in result.get_item("vertices")?.try_iter()? {
        let vertex = vertex?;
        vertices.push(VertexDescriptor {
            position: vertex.get_item("position")?.extract()?,
            neighbours: extract_neighbours(&vertex.get_item("neighbours")?)?,
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

fn extract_neighbours(
    value: &Bound<'_, PyAny>,
) -> PyResult<Vec<cadmark_core::geometry::TopologyElement>> {
    use cadmark_core::geometry::{EdgeId, FaceId, TopologyElement, VertexId};

    value
        .try_iter()?
        .map(|neighbour| {
            let neighbour = neighbour?;
            let id = neighbour.get_item("id")?.extract()?;
            match neighbour.get_item("kind")?.extract::<&str>()? {
                "face" => Ok(TopologyElement::Face(FaceId(id))),
                "edge" => Ok(TopologyElement::Edge(EdgeId(id))),
                "vertex" => Ok(TopologyElement::Vertex(VertexId(id))),
                kind => Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "unknown topology neighbour kind: {kind}"
                ))),
            }
        })
        .collect()
}

/// Whether each solid of the model is closed and valid, in traversal order.
pub(crate) fn solid_validity(
    py: Python<'_>,
    shape: &Bound<'_, PyAny>,
) -> PyResult<Vec<SolidValidity>> {
    let namespace = PyDict::new(py);
    py.run(MEASUREMENT_SOURCE, Some(&namespace), None)?;
    let check = namespace
        .get_item("solid_validity")?
        .expect("measurement source defines solid_validity()");
    let mut validity = Vec::new();
    for solid in check.call1((shape,))?.try_iter()? {
        let solid = solid?;
        validity.push(SolidValidity {
            closed: solid.get_item("closed")?.extract()?,
            valid: solid.get_item("valid")?.extract()?,
        });
    }
    Ok(validity)
}

/// Measure the closest separation of two elements from the model retained by
/// the worker. The IDs are the traversal indices exposed to picking.
pub(crate) fn minimum_distance(
    model: &ModelFile,
    first: &TopologyElement,
    second: &TopologyElement,
) -> Result<MinimumDistance, MeasurementError> {
    let _execution_guard = crate::execution::PYTHON_EXECUTION_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Python::with_gil(|py| {
        let namespace = PyDict::new(py);
        py.run(MEASUREMENT_SOURCE, Some(&namespace), None)?;
        let measure = namespace
            .get_item("minimum_distance")?
            .expect("measurement source defines minimum_distance()");
        let (first_kind, first_index) = element_reference(first)?;
        let (second_kind, second_index) = element_reference(second)?;
        let millimetres = measure
            .call1((
                model.0.display().to_string(),
                first_kind,
                first_index,
                second_kind,
                second_index,
            ))?
            .extract()?;
        Ok(MinimumDistance { millimetres })
    })
}

/// The traversal collection and index the measurement helper looks an element
/// up by. A part names no such collection: distance between whole parts is a
/// relationship between parts, which the commission places outside CADmark.
fn element_reference(
    element: &TopologyElement,
) -> Result<(&'static str, u32), MeasurementError> {
    match element {
        TopologyElement::Face(id) => Ok(("face", id.0)),
        TopologyElement::Edge(id) => Ok(("edge", id.0)),
        TopologyElement::Vertex(id) => Ok(("vertex", id.0)),
        TopologyElement::Part(_) => {
            Err(MeasurementError::NotMeasurable(element.display_label()))
        }
    }
}
