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
    #[error("Tessellation produced no triangles")]
    EmptyMesh,
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
    /// The edge's provenance shape hash for picking correlation.
    pub shape_hash: u64,
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
    /// Shape hashes per face for provenance correlation.
    pub face_shape_hashes: Vec<u64>,
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
import math

def _cadmark_tessellate(shape, linear_deflection=0.1, angular_deflection=0.5):
    \"\"\"Tessellate an OCP shape into vertices, normals, indices, and face IDs.\"\"\"
    mesh = BRepMesh_IncrementalMesh(shape, linear_deflection, False, angular_deflection, True)
    mesh.Perform()

    all_vertices = []
    all_normals = []
    all_indices = []
    all_face_ids = []
    face_shape_hashes = []
    edges = []

    vertex_offset = 0
    face_id = 0

    explorer = TopExp_Explorer(shape, TopAbs_FACE)
    while explorer.More():
        face = explorer.Current()
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

            face_shape_hashes.append(face.HashCode(2**31 - 1))
            vertex_offset += nb_nodes
            face_id += 1

        explorer.Next()

    edge_explorer = TopExp_Explorer(shape, TopAbs_EDGE)
    while edge_explorer.More():
        edge = edge_explorer.Current()
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
                    'shape_hash': edge.HashCode(2**31 - 1),
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
        'face_shape_hashes': face_shape_hashes,
        'edges': edges,
    }
";

/// Code to find the result shape in the script namespace and log
/// diagnostic info about what was found.
///
/// Checks for raw OCP shapes, build123d Shape subclasses, and Builder
/// context managers (BuildPart, BuildSketch, BuildLine) which store
/// their result in `.part`, `.sketch`, or `.line` respectively.
///
/// Sets `_cadmark_result_shape` to the found shape (or None) and
/// `_cadmark_namespace_debug` to a diagnostic string listing all
/// user-defined names and their types.
const FIND_SHAPE_CODE: &std::ffi::CStr = c"
_cadmark_result_shape = None
_cadmark_namespace_debug_entries = []
for _name, _val in list(locals().items()):
    if _name.startswith('_cadmark') or _name.startswith('__'):
        continue

    _type_name = type(_val).__name__
    _cadmark_namespace_debug_entries.append(f'{_name}: {_type_name}')

    # Direct OCP/build123d shape types.
    if _type_name in ('Solid', 'Compound', 'Shell', 'Part', 'Shape', 'Face'):
        _cadmark_result_shape = _val

    # Builder context managers — extract their built result.
    elif _type_name == 'BuildPart':
        if hasattr(_val, 'part') and _val.part is not None:
            _cadmark_result_shape = _val.part
    elif _type_name == 'BuildSketch':
        if hasattr(_val, 'sketch') and _val.sketch is not None:
            _cadmark_result_shape = _val.sketch
    elif _type_name == 'BuildLine':
        if hasattr(_val, 'line') and _val.line is not None:
            _cadmark_result_shape = _val.line

    # Fallback: anything with an OCP .wrapped attribute and a .part accessor.
    elif hasattr(_val, 'wrapped') and hasattr(_val, 'part'):
        _cadmark_result_shape = _val.part

_cadmark_namespace_debug = ', '.join(_cadmark_namespace_debug_entries) if _cadmark_namespace_debug_entries else '(empty)'
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
    // Inject tessellation helper.
    py.run(TESSELLATION_SOURCE, Some(namespace), None)
        .map_err(TessellationError::Python)?;

    // Find the result shape — also captures diagnostic info about
    // what names and types exist in the namespace for debugging.
    py.run(FIND_SHAPE_CODE, Some(namespace), None)
        .map_err(TessellationError::Python)?;

    // Extract the diagnostic string for logging/error messages.
    let namespace_debug: String = namespace
        .get_item("_cadmark_namespace_debug")?
        .map(|v| v.extract().unwrap_or_default())
        .unwrap_or_else(|| "(diagnostic not available)".into());
    log::debug!("Script namespace contents: {namespace_debug}");

    let shape = namespace
        .get_item("_cadmark_result_shape")?
        .ok_or_else(|| {
            log::error!("Shape search variable missing from namespace. Contents: {namespace_debug}");
            TessellationError::NoShape(format!("Namespace contents: {namespace_debug}"))
        })?;

    if shape.is_none() {
        log::error!("No recognised shape in namespace. Contents: {namespace_debug}");
        return Err(TessellationError::NoShape(
            format!("Namespace contents: {namespace_debug}"),
        ));
    }

    log::info!(
        "Found shape: {} (type: {})",
        shape.repr().map(|s| s.to_string()).unwrap_or_else(|_| "?".into()),
        shape.get_type().name().map(|s| s.to_string()).unwrap_or_else(|_| "?".into()),
    );

    // Unwrap build123d objects and tessellate.
    py.run(TESSELLATE_CODE, Some(namespace), None)
        .map_err(TessellationError::Python)?;

    let result = namespace
        .get_item("_cadmark_tess_result")?
        .ok_or_else(|| TessellationError::NoShape(
            "Tessellation produced no result dict".into(),
        ))?;

    parse_tessellation_result(py, &result)
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
    let face_shape_hashes: Vec<u64> = result.get_item("face_shape_hashes")?.extract()?;

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
        let shape_hash: u64 = edge_dict.get_item("shape_hash")?.extract()?;
        edges.push(MeshEdge {
            points: points.iter().map(|p| [p[0], p[1], p[2]]).collect(),
            shape_hash,
        });
    }

    Ok(TessellatedMesh {
        vertices,
        indices,
        face_ids,
        face_shape_hashes,
        edges,
    })
}
