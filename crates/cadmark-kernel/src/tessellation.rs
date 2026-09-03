// Tessellation extraction — converts OCP shapes to triangle meshes
// for the wgpu renderer.

use pyo3::prelude::*;
use pyo3::types::PyDict;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TessellationError {
    #[error("Python error during tessellation: {0}")]
    Python(#[from] PyErr),
    #[error("No shape found in script namespace. {0}")]
    NoShape(String),
    #[error("Unsupported build123d script output. {0}")]
    UnsupportedScriptOutput(String),
    #[error("Tessellation produced no triangles")]
    EmptyMesh,
    #[error("{kind} topology ID {index} is outside final-map bound {bound}")]
    InvalidTopologyId {
        kind: &'static str,
        index: u32,
        bound: usize,
    },
}

/// A vertex in the tessellated mesh.
#[derive(Debug, Clone, Copy)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

/// A tessellated edge for wireframe overlay rendering.
#[derive(Debug, Clone)]
pub struct MeshEdge {
    /// Polyline vertices approximating the edge curve.
    pub points: Vec<[f32; 3]>,
    /// Global zero-based final-topology edge identifier.
    pub edge_id: u32,
}

/// The complete tessellated output from a build123d script.
#[derive(Debug, Default)]
pub struct TessellatedMesh {
    /// Triangle vertices — every 3 consecutive form a triangle.
    pub vertices: Vec<MeshVertex>,
    /// Triangle indices.
    pub indices: Vec<u32>,
    /// Per-triangle face identifier for GPU picking.
    /// Index i corresponds to triangle i (indices[i*3..i*3+3]).
    pub face_ids: Vec<u32>,
    /// Edges for wireframe overlay.
    pub edges: Vec<MeshEdge>,
}

/// Python source for tessellation extraction.
/// Uses OCP's BRepMesh and topology explorers.
const TESSELLATION_SOURCE: &std::ffi::CStr = c"
from OCP.BRepMesh import BRepMesh_IncrementalMesh
from OCP.TopExp import TopExp_Explorer
from OCP.TopAbs import TopAbs_FACE, TopAbs_EDGE
from OCP.BRep import BRep_Tool
from OCP.TopLoc import TopLoc_Location
from OCP.GCPnts import GCPnts_TangentialDeflection
from OCP.BRepAdaptor import BRepAdaptor_Curve
from OCP.TopoDS import TopoDS
import math

def _cadmark_tessellate(shape, linear_deflection=0.1, angular_deflection=0.5):
    \"\"\"Tessellate an OCP shape into vertices, normals, indices, and face IDs.\"\"\"
    mesh = BRepMesh_IncrementalMesh(shape, linear_deflection, False, angular_deflection, True)
    mesh.Perform()

    all_vertices = []
    all_normals = []
    all_indices = []
    all_face_ids = []
    edges = []

    vertex_offset = 0

    explorer = TopExp_Explorer(shape, TopAbs_FACE)
    while explorer.More():
        face = TopoDS.Face_s(explorer.Current())
        face_id = _cadmark_session.topology_index(face, 'face')
        location = TopLoc_Location()
        triangulation = BRep_Tool.Triangulation_s(face, location)

        if triangulation is not None:
            transform = location.Transformation()
            nb_triangles = triangulation.NbTriangles()
            nb_nodes = triangulation.NbNodes()

            for i in range(1, nb_nodes + 1):
                node = triangulation.Node(i)
                node = node.Transformed(transform)
                all_vertices.append([node.X(), node.Y(), node.Z()])

                if triangulation.HasNormals():
                    normal = triangulation.Normal(i)
                    all_normals.append([normal.X(), normal.Y(), normal.Z()])
                else:
                    all_normals.append([0.0, 0.0, 1.0])

            for i in range(1, nb_triangles + 1):
                tri = triangulation.Triangle(i)
                n1, n2, n3 = tri.Get()
                all_indices.extend([
                    n1 - 1 + vertex_offset,
                    n2 - 1 + vertex_offset,
                    n3 - 1 + vertex_offset,
                ])
                all_face_ids.append(face_id)

            vertex_offset += nb_nodes

        explorer.Next()

    edge_explorer = TopExp_Explorer(shape, TopAbs_EDGE)
    while edge_explorer.More():
        edge = TopoDS.Edge_s(edge_explorer.Current())
        try:
            adaptor = BRepAdaptor_Curve(edge)
            deflector = GCPnts_TangentialDeflection(adaptor, angular_deflection, linear_deflection)
            points = []
            for i in range(1, deflector.NbPoints() + 1):
                p = deflector.Value(i)
                points.append([p.X(), p.Y(), p.Z()])
            if points:
                edges.append({
                    'points': points,
                    'edge_id': _cadmark_session.topology_index(edge, 'edge'),
                })
        except Exception:
            # Some edges (seam edges, degenerate edges from boolean ops)
            # cannot be tessellated. Skip them — the mesh renders without
            # those wireframe segments, which is acceptable.
            pass
        edge_explorer.Next()

    return {
        'vertices': all_vertices,
        'normals': all_normals,
        'indices': all_indices,
        'face_ids': all_face_ids,
        'edges': edges,
    }
";

/// Code to unwrap build123d objects and tessellate.
const TESSELLATE_CODE: &std::ffi::CStr = c"
if hasattr(_cadmark_result_shape, 'wrapped'):
    _cadmark_ocp_shape = _cadmark_result_shape.wrapped
else:
    _cadmark_ocp_shape = _cadmark_result_shape
_cadmark_tess_result = _cadmark_tessellate(_cadmark_ocp_shape)
";

/// Extract and tessellate the result shape from a script's namespace.
pub fn tessellate_from_namespace(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
) -> Result<TessellatedMesh, TessellationError> {
    let shape = find_result_shape(namespace)?;

    namespace
        .set_item("_cadmark_result_shape", &shape)
        .map_err(TessellationError::Python)?;

    // Inject tessellation helper after extracting the user result so helper
    // imports don't pollute shape-discovery diagnostics.
    py.run(TESSELLATION_SOURCE, Some(namespace), None)
        .map_err(TessellationError::Python)?;

    log::info!(
        "Found shape: {} (type: {})",
        shape
            .repr()
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "?".into()),
        shape
            .get_type()
            .name()
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "?".into()),
    );

    // Unwrap build123d objects and tessellate.
    py.run(TESSELLATE_CODE, Some(namespace), None)
        .map_err(TessellationError::Python)?;

    let result = namespace
        .get_item("_cadmark_tess_result")?
        .ok_or_else(|| TessellationError::NoShape("Tessellation produced no result dict".into()))?;

    parse_tessellation_result(py, &result)
}

/// The raw OCP shape behind a build123d object (or the object itself when it
/// is already an OCP shape).
pub(crate) fn unwrap_shape<'py>(
    shape: &Bound<'py, PyAny>,
) -> Result<Bound<'py, PyAny>, TessellationError> {
    match get_non_none_attr(shape, "wrapped")? {
        Some(wrapped) => Ok(wrapped),
        None => Ok(shape.clone()),
    }
}

pub(crate) fn find_result_shape<'py>(
    namespace: &Bound<'py, PyDict>,
) -> Result<Bound<'py, PyAny>, TessellationError> {
    let mut namespace_debug = Vec::new();
    let mut build_part_shape = None;
    let mut unsupported_outputs = Vec::new();

    for (name, value) in namespace.iter() {
        let Ok(name) = name.extract::<String>() else {
            continue;
        };
        if is_internal_namespace_name(&name) {
            continue;
        }

        let type_name = value
            .get_type()
            .name()
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "?".into());
        namespace_debug.push(format!("{name}: {type_name}"));

        match type_name.as_str() {
            "BuildPart" => {
                if let Some(part) = get_non_none_attr(&value, "part")? {
                    build_part_shape = Some(part);
                }
            }
            // Builders hold their result in `_obj`. The public `sketch` and
            // `line` properties rebuild the result in global coordinates
            // through OCP copies, which the provenance wrapper refuses
            // outside a user frame, so only the stored object is inspected.
            "BuildSketch" => {
                if get_non_none_attr(&value, "_obj")?.is_some() {
                    unsupported_outputs.push(format!("{name}: BuildSketch (2D output)"));
                }
            }
            "BuildLine" => {
                if get_non_none_attr(&value, "_obj")?.is_some() {
                    unsupported_outputs.push(format!("{name}: BuildLine (1D output)"));
                }
            }
            _ => {
                if has_non_none_attr(&value, "wrapped")? {
                    unsupported_outputs.push(format!("{name}: {type_name}"));
                }
            }
        }
    }

    let namespace_debug = if namespace_debug.is_empty() {
        "(empty)".to_string()
    } else {
        namespace_debug.join(", ")
    };
    log::debug!("Script namespace contents: {namespace_debug}");

    if let Some(shape) = build_part_shape {
        return Ok(shape);
    }

    if !unsupported_outputs.is_empty() {
        let details = unsupported_outputs.join(", ");
        log::error!(
            "Unsupported script output detected. Outputs: {details}. Namespace contents: {namespace_debug}"
        );
        return Err(TessellationError::UnsupportedScriptOutput(format!(
            "CADmark only supports Builder mode scripts with a completed `BuildPart` context. \
             Unsupported outputs detected: {details}. \
             Use `with BuildPart() as part:` and leave the final 3D model in `part.part`. \
             Namespace contents: {namespace_debug}"
        )));
    }

    log::error!("No completed BuildPart found. Contents: {namespace_debug}");
    Err(TessellationError::NoShape(format!(
        "Expected a completed `BuildPart` context such as `with BuildPart() as part:`. \
         Namespace contents: {namespace_debug}"
    )))
}

fn is_internal_namespace_name(name: &str) -> bool {
    matches!(
        name,
        "_cadmark_tessellate"
            | "_cadmark_result_shape"
            | "_cadmark_ocp_shape"
            | "_cadmark_tess_result"
            | "BRepMesh_IncrementalMesh"
            | "TopExp_Explorer"
            | "TopAbs_FACE"
            | "TopAbs_EDGE"
            | "BRep_Tool"
            | "TopLoc_Location"
            | "GCPnts_TangentialDeflection"
            | "BRepAdaptor_Curve"
            | "TopoDS"
            | "math"
    ) || name.starts_with("_cadmark")
        || name.starts_with("__")
}

fn get_non_none_attr<'py>(
    value: &Bound<'py, PyAny>,
    attr: &str,
) -> Result<Option<Bound<'py, PyAny>>, TessellationError> {
    match value.getattr(attr) {
        Ok(attr_value) if !attr_value.is_none() => Ok(Some(attr_value)),
        Ok(_) => Ok(None),
        Err(err) if err.is_instance_of::<pyo3::exceptions::PyAttributeError>(value.py()) => {
            Ok(None)
        }
        Err(err) => Err(TessellationError::Python(err)),
    }
}

fn has_non_none_attr(value: &Bound<'_, PyAny>, attr: &str) -> Result<bool, TessellationError> {
    Ok(get_non_none_attr(value, attr)?.is_some())
}

/// Parse the Python tessellation dict into a Rust struct.
fn parse_tessellation_result(
    _py: Python<'_>,
    result: &Bound<'_, PyAny>,
) -> Result<TessellatedMesh, TessellationError> {
    let vertices_raw: Vec<Vec<f32>> = result.get_item("vertices")?.extract()?;
    let normals_raw: Vec<Vec<f32>> = result.get_item("normals")?.extract()?;
    let indices: Vec<u32> = result.get_item("indices")?.extract()?;
    let face_ids: Vec<u32> = result.get_item("face_ids")?.extract()?;

    if indices.is_empty() {
        return Err(TessellationError::EmptyMesh);
    }

    let vertices: Vec<MeshVertex> = vertices_raw
        .iter()
        .zip(normals_raw.iter())
        .map(|(pos, norm)| MeshVertex {
            position: [pos[0], pos[1], pos[2]],
            normal: [norm[0], norm[1], norm[2]],
        })
        .collect();

    // Parse edges.
    let edges_raw: Vec<Bound<'_, PyAny>> = result.get_item("edges")?.extract()?;
    let mut edges = Vec::with_capacity(edges_raw.len());
    for edge_dict in &edges_raw {
        let points: Vec<Vec<f32>> = edge_dict.get_item("points")?.extract()?;
        let edge_id: u32 = edge_dict.get_item("edge_id")?.extract()?;
        edges.push(MeshEdge {
            points: points.iter().map(|p| [p[0], p[1], p[2]]).collect(),
            edge_id,
        });
    }

    Ok(TessellatedMesh {
        vertices,
        indices,
        face_ids,
        edges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_completed_build_part_output() {
        Python::with_gil(|py| {
            let namespace = PyDict::new(py);
            py.run(
                c"
class DirectShape:
    def __init__(self):
        self.wrapped = object()

class BuildPart:
    def __init__(self):
        self.part = DirectShape()

part = BuildPart()
",
                Some(&namespace),
                None,
            )
            .unwrap();

            let shape = find_result_shape(&namespace).unwrap();
            assert!(shape.getattr("wrapped").is_ok());
        });
    }

    #[test]
    fn rejects_direct_shape_outputs_without_build_part() {
        Python::with_gil(|py| {
            let namespace = PyDict::new(py);
            py.run(
                c"
class DirectShape:
    def __init__(self):
        self.wrapped = object()

result = DirectShape()
",
                Some(&namespace),
                None,
            )
            .unwrap();

            let err = find_result_shape(&namespace).unwrap_err();
            assert!(matches!(err, TessellationError::UnsupportedScriptOutput(_)));
            assert!(
                err.to_string()
                    .contains("CADmark only supports Builder mode scripts")
            );
        });
    }

    #[test]
    fn rejects_sketch_only_builder_scripts() {
        Python::with_gil(|py| {
            let namespace = PyDict::new(py);
            py.run(
                c"
class DirectShape:
    def __init__(self):
        self.wrapped = object()

class BuildSketch:
    def __init__(self):
        self._obj = DirectShape()

sketch = BuildSketch()
",
                Some(&namespace),
                None,
            )
            .unwrap();

            let err = find_result_shape(&namespace).unwrap_err();
            assert!(matches!(err, TessellationError::UnsupportedScriptOutput(_)));
            assert!(err.to_string().contains("BuildSketch"));
        });
    }

    #[test]
    fn reports_missing_build_part_when_no_shape_exists() {
        Python::with_gil(|py| {
            let namespace = PyDict::new(py);
            py.run(c"value = 42", Some(&namespace), None).unwrap();

            let err = find_result_shape(&namespace).unwrap_err();
            assert!(matches!(err, TessellationError::NoShape(_)));
            assert!(
                err.to_string()
                    .contains("Expected a completed `BuildPart` context")
            );
        });
    }

    #[test]
    fn tessellation_injects_result_shape_before_running_helper_code() {
        Python::with_gil(|py| {
            let namespace = PyDict::new(py);
            py.run(
                c"
class DirectShape:
    def __init__(self):
        self.wrapped = object()

class BuildPart:
    def __init__(self):
        self.part = DirectShape()

part = BuildPart()
",
                Some(&namespace),
                None,
            )
            .unwrap();

            let shape = find_result_shape(&namespace).unwrap();
            namespace.set_item("_cadmark_result_shape", &shape).unwrap();
            let err = py.run(TESSELLATE_CODE, Some(&namespace), None).unwrap_err();
            assert!(!err.to_string().contains("_cadmark_result_shape"));

            let stored_shape = namespace
                .get_item("_cadmark_result_shape")
                .unwrap()
                .unwrap();
            assert_eq!(stored_shape.get_type().name().unwrap(), "DirectShape");
        });
    }

    #[test]
    fn missing_build_part_error_only_reports_user_namespace() {
        Python::with_gil(|py| {
            let namespace = PyDict::new(py);
            py.run(
                c"
value = 42
BRepMesh_IncrementalMesh = object()
math = __import__('math')
",
                Some(&namespace),
                None,
            )
            .unwrap();

            let err = find_result_shape(&namespace).unwrap_err();
            let msg = err.to_string();

            assert!(msg.contains("value: int"));
            assert!(!msg.contains("BRepMesh_IncrementalMesh"));
            assert!(!msg.contains("math: module"));
        });
    }
}
