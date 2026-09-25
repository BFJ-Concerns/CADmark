// Tessellation extraction — converts OCP shapes to the kernel-neutral
// triangle mesh the renderer consumes.

use cadmark_core::mesh::{MeshEdge, MeshVertex, TessellatedMesh};
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

/// Both solid surfaces and sketch curves use a scale-relative display
/// tolerance. Exact modelling and export geometry are unaffected.
pub(crate) fn prepare_display_tessellation(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
) -> Result<(), TessellationError> {
    crate::bounds::define_exact_bounds(py, namespace).map_err(TessellationError::Python)?;
    py.run(
        c"
def _cadmark_display_deflection(shape):
    box = _cadmark_exact_bounds(shape)
    if box.IsVoid():
        return 1e-7, 0.1
    bounds = box.Get()
    span = max(bounds[axis + 3] - bounds[axis] for axis in range(3))
    # At a 1000-pixel fit, chord error is about 0.1 pixels. The angular
    # bound preserves small curved details within a much larger model.
    return max(span * 1e-4, 1e-7), 0.1
",
        Some(namespace),
        None,
    )
    .map_err(TessellationError::Python)
}

/// Python source for tessellation extraction.
/// Uses OCP's BRepMesh and topology explorers.
const TESSELLATION_SOURCE: &std::ffi::CStr = c"
from OCP.BRepLib import BRepLib
from OCP.BRepMesh import BRepMesh_IncrementalMesh
from OCP.TopExp import TopExp_Explorer
from OCP.TopAbs import TopAbs_FACE, TopAbs_EDGE, TopAbs_REVERSED
from OCP.BRep import BRep_Tool
from OCP.gp import gp_Vec
from OCP.TopLoc import TopLoc_Location
from OCP.GCPnts import GCPnts_TangentialDeflection
from OCP.BRepAdaptor import BRepAdaptor_Curve
from OCP.TopoDS import TopoDS
import math

def _cadmark_node_normal(triangulation, index, transform):
    if not triangulation.HasNormals():
        return None
    try:
        normal = gp_Vec(triangulation.Normal(index)).Transformed(transform)
    except Exception:
        return None
    return [normal.X(), normal.Y(), normal.Z()]


def _cadmark_tessellate(shape):
    \"\"\"Tessellate an OCP shape into vertices, normals, indices, and face IDs.\"\"\"
    linear_deflection, angular_deflection = _cadmark_display_deflection(shape)
    mesh = BRepMesh_IncrementalMesh(shape, linear_deflection, False, angular_deflection, True)
    mesh.Perform()
    # Meshing alone stores no per-node normals; this derives them from the
    # surfaces so curved faces shade smoothly instead of flat. The stored
    # normals already honour each face's orientation, so a reversed face
    # points outward without further correction.
    BRepLib.EnsureNormalConsistency_s(shape)

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
            # Triangles are wound anticlockwise about the surface normal. A
            # reversed face's outward normal opposes the surface, so its
            # triangles are re-wound to keep every front face anticlockwise.
            reversed_face = face.Orientation() == TopAbs_REVERSED

            face_normals = []
            for i in range(1, nb_nodes + 1):
                node = triangulation.Node(i)
                node = node.Transformed(transform)
                all_vertices.append([node.X(), node.Y(), node.Z()])
                face_normals.append(_cadmark_node_normal(triangulation, i, transform))

            # A node on a surface singularity (a loft apex, a sweep seam)
            # stores no usable normal; give it the face's average so the
            # triangle still shades rather than going black.
            valid = [n for n in face_normals if n is not None]
            if valid:
                average = [sum(n[axis] for n in valid) / len(valid) for axis in range(3)]
                length = math.sqrt(sum(component * component for component in average)) or 1.0
                average = [component / length for component in average]
            else:
                average = [0.0, 0.0, 1.0]
            all_normals.extend(n if n is not None else average for n in face_normals)

            for i in range(1, nb_triangles + 1):
                tri = triangulation.Triangle(i)
                n1, n2, n3 = tri.Get()
                if reversed_face:
                    n2, n3 = n3, n2
                all_indices.extend([
                    n1 - 1 + vertex_offset,
                    n2 - 1 + vertex_offset,
                    n3 - 1 + vertex_offset,
                ])
                all_face_ids.append(face_id)

            vertex_offset += nb_nodes

        explorer.Next()

    seen_edges = set()
    edge_explorer = TopExp_Explorer(shape, TopAbs_EDGE)
    while edge_explorer.More():
        edge = TopoDS.Edge_s(edge_explorer.Current())
        edge_id = _cadmark_session.topology_index(edge, 'edge')
        edge_explorer.Next()
        if edge_id in seen_edges:
            continue
        seen_edges.add(edge_id)
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
                    'edge_id': edge_id,
                })
        except Exception:
            # Some edges (seam edges, degenerate edges from boolean ops)
            # cannot be tessellated. Skip them — the mesh renders without
            # those wireframe segments, which is acceptable.
            pass

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

/// Tessellate a particular discovered result while reusing the script
/// namespace that owns the helper functions.
pub(crate) fn tessellate_shape(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    shape: &Bound<'_, PyAny>,
) -> Result<TessellatedMesh, TessellationError> {
    namespace
        .set_item("_cadmark_result_shape", shape)
        .map_err(TessellationError::Python)?;

    // Inject tessellation helper after extracting the user result so helper
    // imports don't pollute shape-discovery diagnostics.
    prepare_display_tessellation(py, namespace)?;
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

/// What a script left at the top level of its namespace.
#[derive(Debug)]
pub(crate) enum ScriptResult<'py> {
    /// Every completed 3D result, in source binding order, each with the
    /// name that names it: the shape's own `label` when the script set
    /// one, otherwise the binding — a completed `BuildPart`'s part, or a
    /// `Part`, `Solid`, or `Compound` bound at the top level. Names are
    /// unique within one result. Never empty.
    Solids(Vec<(String, Bound<'py, PyAny>)>),
    /// A sketch or a line and no solid: the script has drawn a profile and
    /// not yet made anything of it. Its own kind of result, not a failure.
    Sketch(Bound<'py, PyAny>),
}

/// The result of a script, whichever build123d idiom wrote it. Every
/// distinct completed solid is a part of its own, so a script may leave
/// several; repeated bindings of the same shape are aliases rather than
/// extra parts, and the last alias supplies the user-facing name unless
/// the shape carries a `label`, which names it under any binding. Python
/// has already resolved repeated bindings of one name before this point,
/// so staged construction should rebind that name rather than leave
/// intermediate shapes bound. Solids win over sketches however they are
/// ordered — a script that sketches a profile and extrudes it has reached
/// a solid — and only a script with no solid at all is a sketch result.
pub(crate) fn find_result_shape<'py>(
    namespace: &Bound<'py, PyDict>,
) -> Result<ScriptResult<'py>, TessellationError> {
    let mut namespace_debug = Vec::new();
    let mut solids: Vec<(String, Bound<'py, PyAny>)> = Vec::new();
    let mut sketches: Vec<(String, Bound<'py, PyAny>)> = Vec::new();

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
                    add_discovered_shape(&mut solids, name, part)?;
                }
            }
            // Builders hold their result in `_obj`. The public `sketch` and
            // `line` properties rebuild the result in global coordinates
            // through OCP copies, which the provenance wrapper refuses
            // outside a user frame, so only the stored object is inspected.
            "BuildSketch" | "BuildLine" | "Sketch" | "Curve" | "Face" | "Wire" | "Edge" => {
                let held = if type_name.starts_with("Build") {
                    get_non_none_attr(&value, "_obj")?.is_some()
                } else {
                    has_non_none_attr(&value, "wrapped")?
                };
                if held {
                    sketches.push((name, value));
                }
            }
            "Part" | "Solid" | "Compound" if has_non_none_attr(&value, "wrapped")? => {
                add_discovered_shape(&mut solids, name, value)?;
            }
            _ => {}
        }
    }

    let namespace_debug = if namespace_debug.is_empty() {
        "(empty)".to_string()
    } else {
        namespace_debug.join(", ")
    };
    log::debug!("Script namespace contents: {namespace_debug}");

    // Dict iteration is insertion order, so the parts come out in the order
    // the script bound them.
    if !solids.is_empty() {
        disambiguate_names(&mut solids);
        log::debug!(
            "Result parts: {}",
            solids
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(ScriptResult::Solids(solids));
    }

    if let Some((name, sketch)) = sketches.pop() {
        log::debug!("Result is the sketch `{name}`");
        return Ok(ScriptResult::Sketch(sketch));
    }

    log::error!("No 3D result found. Contents: {namespace_debug}");
    Err(TessellationError::NoShape(format!(
        "Expected a 3D result at the top level of the script: a completed `BuildPart`, \
         or a `Part`, `Solid`, or `Compound` bound to a name. Namespace contents: {namespace_debug}"
    )))
}

fn add_discovered_shape<'py>(
    solids: &mut Vec<(String, Bound<'py, PyAny>)>,
    binding: String,
    shape: Bound<'py, PyAny>,
) -> Result<(), TessellationError> {
    let name = part_name(binding, &shape)?;
    for (existing_name, existing_shape) in solids.iter_mut() {
        if same_shape(existing_shape, &shape)? {
            *existing_name = name;
            return Ok(());
        }
    }
    solids.push((name, shape));
    Ok(())
}

/// The name a part goes by: the `label` build123d lets a script give any
/// shape, when the script set one, otherwise the binding the shape was
/// found under. The label is how a script names a part deliberately —
/// `lid.label = "lid"`, or `bp.part.label = "lid"` after a builder — and
/// it holds under whatever binding the shape ends up in.
fn part_name(binding: String, shape: &Bound<'_, PyAny>) -> Result<String, TessellationError> {
    if let Some(label) = get_non_none_attr(shape, "label")?
        && let Ok(label) = label.extract::<String>()
    {
        let label = label.trim();
        if !label.is_empty() {
            return Ok(label.to_string());
        }
    }
    Ok(binding)
}

/// Make every part's name unique in discovery order: a later part whose
/// name repeats an earlier one's is numbered (`lid`, `lid (2)`). A name
/// is a part's identity from one execution to the next — the visibility
/// the user set and the part the AI asks to render are both kept by it —
/// so two parts never share one.
fn disambiguate_names(solids: &mut [(String, Bound<'_, PyAny>)]) {
    let mut taken = std::collections::HashSet::new();
    for (name, _) in solids.iter_mut() {
        if taken.contains(name) {
            let mut ordinal = 2;
            while taken.contains(&format!("{name} ({ordinal})")) {
                ordinal += 1;
            }
            *name = format!("{name} ({ordinal})");
        }
        taken.insert(name.clone());
    }
}

fn same_shape(
    left: &Bound<'_, PyAny>,
    right: &Bound<'_, PyAny>,
) -> Result<bool, TessellationError> {
    if left.as_ptr() == right.as_ptr() {
        return Ok(true);
    }
    match (
        get_non_none_attr(left, "wrapped")?,
        get_non_none_attr(right, "wrapped")?,
    ) {
        (Some(left), Some(right)) => Ok(left.as_ptr() == right.as_ptr()),
        _ => Ok(false),
    }
}

fn is_internal_namespace_name(name: &str) -> bool {
    matches!(
        name,
        "_cadmark_tessellate"
            | "_cadmark_result_shape"
            | "_cadmark_ocp_shape"
            | "_cadmark_tess_result"
            | "_cadmark_node_normal"
            | "BRepLib"
            | "BRepMesh_IncrementalMesh"
            | "TopExp_Explorer"
            | "TopAbs_FACE"
            | "TopAbs_EDGE"
            | "TopAbs_REVERSED"
            | "BRep_Tool"
            | "gp_Vec"
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

    /// A stand-in for a build123d shape: something with a `wrapped`.
    const FAKE_SHAPES: &std::ffi::CStr = c"
class _Wrapped:
    def __init__(self):
        self.wrapped = object()

class Part(_Wrapped): pass
class Solid(_Wrapped): pass
class Compound(_Wrapped): pass
class Sketch(_Wrapped): pass
class Face(_Wrapped): pass

class BuildPart:
    def __init__(self, part=None):
        self.part = part

class BuildSketch:
    def __init__(self, obj=None):
        self._obj = obj
";

    fn namespace_from<'py>(py: Python<'py>, script: &std::ffi::CStr) -> Bound<'py, PyDict> {
        let namespace = PyDict::new(py);
        py.run(FAKE_SHAPES, Some(&namespace), None).unwrap();
        py.run(script, Some(&namespace), None).unwrap();
        namespace
    }

    /// Every part a script was expected to leave, failing the test when it
    /// left a sketch instead.
    fn discover<'py>(
        namespace: &Bound<'py, PyDict>,
    ) -> Result<Vec<(String, Bound<'py, PyAny>)>, TessellationError> {
        match find_result_shape(namespace)? {
            ScriptResult::Solids(shapes) => Ok(shapes),
            ScriptResult::Sketch(_) => panic!("expected a solid result, got a sketch"),
        }
    }

    /// The single solid a script was expected to leave.
    fn solid<'py>(result: ScriptResult<'py>) -> Bound<'py, PyAny> {
        match result {
            ScriptResult::Solids(mut shapes) => {
                assert_eq!(shapes.len(), 1, "expected exactly one part");
                shapes.pop().expect("one part").1
            }
            ScriptResult::Sketch(_) => panic!("expected a solid result, got a sketch"),
        }
    }

    #[test]
    fn production_discovery_keeps_a_completed_build_part() {
        Python::with_gil(|py| {
            let namespace = namespace_from(py, c"part = BuildPart(Part())");
            let shapes = discover(&namespace).unwrap();
            assert_eq!(shapes.len(), 1);
            assert_eq!(shapes[0].0, "part");
            assert_eq!(shapes[0].1.get_type().name().unwrap(), "Part");
        });
    }

    #[test]
    fn production_discovery_keeps_distinct_builder_direct_and_algebra_parts() {
        Python::with_gil(|py| {
            for script in [
                c"result = Part()",
                c"result = Solid()",
                c"result = Compound()",
            ] {
                let namespace = namespace_from(py, script);
                assert_eq!(discover(&namespace).unwrap().len(), 1);
            }
            let namespace = namespace_from(py, c"first = Part()\nsecond = Solid()");
            let shapes = discover(&namespace).unwrap();
            assert_eq!(
                shapes
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
                ["first", "second"]
            );
            let namespace = namespace_from(py, c"bp = BuildPart(Part())\nfinal = Compound()");
            let shapes = discover(&namespace).unwrap();
            assert_eq!(
                shapes
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
                ["bp", "final"]
            );
        });
    }

    #[test]
    fn production_discovery_coalesces_an_alias_without_losing_other_parts() {
        Python::with_gil(|py| {
            let namespace = namespace_from(
                py,
                c"bracket = BuildPart(Part())\ncap = BuildPart(Part())\nresult = cap.part",
            );
            let shapes = discover(&namespace).unwrap();
            assert_eq!(shapes.len(), 2);
            assert_eq!(shapes[0].0, "bracket");
            assert_eq!(shapes[1].0, "result");
        });
    }

    #[test]
    fn production_discovery_keeps_the_final_value_of_a_rebound_name() {
        Python::with_gil(|py| {
            let namespace = namespace_from(py, c"part = Part()\npart = Part()");
            let shapes = discover(&namespace).unwrap();
            assert_eq!(shapes.len(), 1);
            assert_eq!(shapes[0].0, "part");
        });
    }

    #[test]
    fn a_sketch_only_script_is_a_sketch_result_not_a_failure() {
        Python::with_gil(|py| {
            for (script, expected) in [
                (c"sketch = BuildSketch(Sketch())", "BuildSketch"),
                (c"profile = Sketch()", "Sketch"),
                (c"f = Face()", "Face"),
            ] {
                let namespace = namespace_from(py, script);
                match find_result_shape(&namespace).unwrap() {
                    ScriptResult::Sketch(sketch) => {
                        assert_eq!(sketch.get_type().name().unwrap(), expected)
                    }
                    ScriptResult::Solids(_) => panic!("{script:?} produced no solid"),
                }
            }
        });
    }

    #[test]
    fn a_sketch_extruded_into_a_solid_is_a_solid_result() {
        Python::with_gil(|py| {
            // Both bindings survive to the end of a builder script; the
            // solid is the result whichever order they were bound in.
            let namespace = namespace_from(py, c"s = BuildSketch(Sketch())\np = Part()");
            assert_eq!(
                solid(find_result_shape(&namespace).unwrap())
                    .get_type()
                    .name()
                    .unwrap(),
                "Part"
            );
            let namespace = namespace_from(py, c"p = Part()\ns = BuildSketch(Sketch())");
            assert_eq!(
                solid(find_result_shape(&namespace).unwrap())
                    .get_type()
                    .name()
                    .unwrap(),
                "Part"
            );
        });
    }

    #[test]
    fn reports_missing_result_when_no_shape_exists() {
        Python::with_gil(|py| {
            let namespace = namespace_from(py, c"value = 42");
            let err = discover(&namespace).unwrap_err();
            assert!(matches!(err, TessellationError::NoShape(_)));
            assert!(err.to_string().contains("Expected a 3D result"));
            assert!(err.to_string().contains("value: int"));
        });
    }

    #[test]
    fn tessellation_injects_result_shape_before_running_helper_code() {
        Python::with_gil(|py| {
            let namespace = namespace_from(py, c"part = BuildPart(Part())");
            let shape = solid(find_result_shape(&namespace).unwrap());
            namespace.set_item("_cadmark_result_shape", &shape).unwrap();
            let err = py.run(TESSELLATE_CODE, Some(&namespace), None).unwrap_err();
            assert!(!err.to_string().contains("_cadmark_result_shape"));

            let stored_shape = namespace
                .get_item("_cadmark_result_shape")
                .unwrap()
                .unwrap();
            assert_eq!(stored_shape.get_type().name().unwrap(), "Part");
        });
    }

    #[test]
    fn missing_result_error_only_reports_user_namespace() {
        Python::with_gil(|py| {
            let namespace = namespace_from(
                py,
                c"
value = 42
BRepMesh_IncrementalMesh = object()
math = __import__('math')
",
            );
            let err = discover(&namespace).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("value: int"));
            assert!(!msg.contains("BRepMesh_IncrementalMesh"));
            assert!(!msg.contains("math: module"));
        });
    }
}
