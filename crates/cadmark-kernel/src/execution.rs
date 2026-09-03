// Script execution — runs build123d Python scripts and captures results.

use std::path::Path;
use std::sync::{Mutex, Once};

use cadmark_core::geometry::{GeometryDescriptors, ModelSummary};
use cadmark_core::ledger::ProvenanceLedger;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use thiserror::Error;

use crate::export::ModelHandle;
use crate::tessellation::TessellatedMesh;

#[derive(Error, Debug)]
pub enum ExecutionError {
    #[error("Python error: {0}")]
    Python(#[from] PyErr),
    /// A failure raised by the user's script, formatted as the traceback the
    /// script author (or the AI) needs to fix it.
    #[error("{0}")]
    Script(String),
    #[error("Script not found: {0}")]
    ScriptNotFound(String),
    #[error("No solid produced by script")]
    NoSolid,
    #[error("Provenance error: {0}")]
    Provenance(#[from] crate::provenance::ProvenanceError),
    #[error("Tessellation error: {0}")]
    Tessellation(#[from] crate::tessellation::TessellationError),
}

/// One-time venv activation guard.
static VENV_ACTIVATED: Once = Once::new();
/// OCP calls can release the GIL, so the process-global binding patch needs
/// an outer Rust lock for the complete execution lifecycle.
pub(crate) static PYTHON_EXECUTION_LOCK: Mutex<()> = Mutex::new(());

/// Activate a Python virtualenv for the embedded interpreter.
///
/// Adds the venv's site-packages to `sys.path` so that packages
/// installed in the venv (e.g. build123d) are importable. Call
/// once at startup — repeated calls are no-ops.
pub fn activate_venv(venv_path: &Path) -> Result<(), ExecutionError> {
    crate::python_runtime::configure_python_home();

    // Glob for the site-packages directory rather than hardcoding
    // the Python minor version — works across 3.x variants.
    let lib_dir = venv_path.join("lib");
    let site_packages = std::fs::read_dir(&lib_dir)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(Result::ok)
                .find(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.starts_with("python3"))
                })
                .map(|e| e.path().join("site-packages"))
        })
        .filter(|p| p.is_dir());

    let Some(site_packages) = site_packages else {
        log::warn!("No site-packages found in venv at {}", venv_path.display());
        return Ok(());
    };

    let site_str = site_packages.to_string_lossy().to_string();

    VENV_ACTIVATED.call_once(|| {
        Python::with_gil(|py| {
            let sys = py.import("sys").expect("failed to import sys");
            let path = sys.getattr("path").expect("no sys.path");
            // Prepend so venv packages shadow system packages.
            path.call_method1("insert", (0, &site_str))
                .expect("failed to insert into sys.path");
            log::info!("Activated venv site-packages: {site_str}");
        });
    });

    Ok(())
}

/// Discover and activate the project venv.
///
/// Search order:
/// 1. `VIRTUAL_ENV` environment variable
/// 2. `.venv/` in the workspace root (baked in at compile time)
/// 3. `.venv/` near the running executable (walk up 4 levels)
/// 4. `.venv/` in the current working directory
pub fn discover_and_activate_venv() -> Result<(), ExecutionError> {
    crate::python_runtime::configure_python_home();

    // VIRTUAL_ENV — set by shell activation or launch scripts.
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        let path = Path::new(&venv);
        if path.is_dir() {
            return activate_venv(path);
        }
    }

    // Compile-time workspace root: CARGO_MANIFEST_DIR points at
    // crates/cadmark-kernel/, so the workspace root is two levels up.
    // This survives installation — the path is baked into the binary.
    {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        if let Some(workspace_root) = manifest_dir.parent().and_then(|p| p.parent()) {
            let candidate = workspace_root.join(".venv");
            if candidate.is_dir() {
                return activate_venv(&candidate);
            }
        }
    }

    // .venv near the executable — walk up from the binary's directory
    // to handle target/debug/ during development.
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        for ancestor in exe_dir.ancestors().take(4) {
            let candidate = ancestor.join(".venv");
            if candidate.is_dir() {
                return activate_venv(&candidate);
            }
        }
    }

    // .venv in cwd — fallback for `cargo run` from the workspace root.
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join(".venv");
        if candidate.is_dir() {
            return activate_venv(&candidate);
        }
    }

    log::warn!("No Python venv found — build123d may not be importable");
    Ok(())
}

/// Result of executing a build123d script.
#[derive(Debug)]
pub struct ExecutionResult {
    /// Tessellated mesh for the renderer.
    pub mesh: TessellatedMesh,
    /// Complete kernel-neutral provenance ledger.
    pub ledger: ProvenanceLedger,
    /// Measured geometry of every element, in ledger order.
    pub descriptors: GeometryDescriptors,
    /// Whole-model measurements.
    pub summary: ModelSummary,
    /// The built model, retained for export.
    pub model: ModelHandle,
}

/// Execute a build123d script and return the tessellated result.
///
/// This function acquires the GIL and must not be called from the
/// UI or rendering thread.
pub fn execute_script(script_path: &Path) -> Result<ExecutionResult, ExecutionError> {
    if !script_path.exists() {
        return Err(ExecutionError::ScriptNotFound(
            script_path.display().to_string(),
        ));
    }

    let script_content = std::fs::read_to_string(script_path)
        .map_err(|e| ExecutionError::ScriptNotFound(e.to_string()))?;

    execute_script_source_named(&script_content, &script_path.display().to_string())
}

/// Execute build123d source code directly (for testing and AI-generated code).
pub fn execute_script_source(source: &str) -> Result<ExecutionResult, ExecutionError> {
    execute_script_source_named(source, "<cadmark-source>")
}

fn execute_script_source_named(
    source: &str,
    filename: &str,
) -> Result<ExecutionResult, ExecutionError> {
    crate::python_runtime::configure_python_home();
    let _execution_guard = PYTHON_EXECUTION_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let line_count = source.lines().count();
    let preview: String = source.lines().take(3).collect::<Vec<_>>().join(" | ");
    log::info!(
        "Executing script ({} bytes, {} lines). First lines: {}",
        source.len(),
        line_count,
        preview,
    );

    Python::with_gil(|py| {
        // Log the Python version and sys.path for environment diagnostics.
        if log::log_enabled!(log::Level::Debug)
            && let Ok(sys) = py.import("sys")
        {
            if let Ok(version) = sys.getattr("version") {
                log::debug!("Python version: {version}");
            }
            if let Ok(path) = sys.getattr("path") {
                log::debug!("sys.path: {path}");
            }
        }

        // Instrumentation and user code intentionally share one namespace.
        let globals = PyDict::new(py);
        let session = crate::provenance::install_instrumentation(py, &globals, filename)?;

        let execution = (|| {
            let builtins = py.import("builtins")?;
            let code = builtins.call_method1("compile", (source, filename, "exec"))?;
            builtins
                .call_method1("exec", (&code, &globals, &globals))
                .map_err(|error| {
                    let message = format_script_error(py, &error, filename);
                    log::error!("Python execution failed: {message}");
                    ExecutionError::Script(message)
                })?;

            let keys: Vec<String> = globals
                .keys()
                .into_iter()
                .filter_map(|key| key.extract::<String>().ok())
                .filter(|key| !key.starts_with("__") && !key.starts_with("_cadmark"))
                .collect();
            log::info!(
                "Script executed. Namespace has {} user entries: [{}]",
                keys.len(),
                keys.join(", "),
            );

            let shape = crate::tessellation::find_result_shape(&globals)?;
            let (_raw, ledger) = crate::provenance::finalise(py, &session, &shape, source)?;
            let mesh = crate::tessellation::tessellate_from_namespace(py, &globals)?;
            validate_tessellation_ids(&mesh, &ledger)?;
            let ocp_shape = crate::tessellation::unwrap_shape(&shape)?;
            let (descriptors, summary) =
                crate::measurement::measure(py, &ocp_shape, session.bound(py))?;
            log::info!("Model measured: {}", summary.describe());
            Ok(ExecutionResult {
                mesh,
                ledger,
                descriptors,
                summary,
                model: ModelHandle::new(shape.unbind()),
            })
        })();

        let restoration = crate::provenance::restore(py, &session);
        if let Err(error) = restoration {
            return Err(ExecutionError::Provenance(error));
        }
        execution
    })
}

/// Render a script failure as the traceback frames that belong to the user's
/// script, so the reader sees their own line numbers rather than build123d
/// internals. Falls back to the bare exception when no user frame exists.
fn format_script_error(py: Python<'_>, error: &PyErr, filename: &str) -> String {
    let rendered = (|| -> PyResult<String> {
        let traceback = py.import("traceback")?;
        let exception = error.value(py);
        let frames: Vec<String> = match error.traceback(py) {
            Some(trace) => traceback
                .call_method1("extract_tb", (trace,))?
                .try_iter()?
                .map(|frame| -> PyResult<Option<String>> {
                    let frame = frame?;
                    let frame_file: String = frame.getattr("filename")?.extract()?;
                    if frame_file != filename {
                        return Ok(None);
                    }
                    let line: u32 = frame.getattr("lineno")?.extract()?;
                    let code: Option<String> = frame.getattr("line")?.extract()?;
                    Ok(Some(match code {
                        Some(code) if !code.trim().is_empty() => {
                            format!("  line {line}: {}", code.trim())
                        }
                        _ => format!("  line {line}"),
                    }))
                })
                .filter_map(|frame| frame.transpose())
                .collect::<PyResult<_>>()?,
            None => Vec::new(),
        };
        let summary: String = traceback
            .call_method1("format_exception_only", (exception.get_type(), exception))?
            .try_iter()?
            .map(|line| line?.extract::<String>())
            .collect::<PyResult<Vec<_>>>()?
            .concat();
        Ok(if frames.is_empty() {
            summary.trim_end().to_string()
        } else {
            format!("{}\n{}", frames.join("\n"), summary.trim_end())
        })
    })();
    rendered.unwrap_or_else(|_| error.to_string())
}

fn validate_tessellation_ids(
    mesh: &TessellatedMesh,
    ledger: &ProvenanceLedger,
) -> Result<(), ExecutionError> {
    if let Some(face_id) = mesh
        .face_ids
        .iter()
        .copied()
        .find(|face_id| *face_id as usize >= ledger.face_count())
    {
        return Err(crate::tessellation::TessellationError::InvalidTopologyId {
            kind: "face",
            index: face_id,
            bound: ledger.face_count(),
        }
        .into());
    }
    if let Some(edge_id) = mesh
        .edges
        .iter()
        .map(|edge| edge.edge_id)
        .find(|edge_id| *edge_id as usize >= ledger.edge_count())
    {
        return Err(crate::tessellation::TessellationError::InvalidTopologyId {
            kind: "edge",
            index: edge_id,
            bound: ledger.edge_count(),
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadmark_core::context::{NullIdentification, resolve_context};
    use cadmark_core::geometry::GeometryContext;
    use cadmark_core::geometry::{EdgeId, FaceId, TopologyElement, VertexId};
    use cadmark_core::ledger::{
        LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
    };

    fn activate_test_runtime() {
        discover_and_activate_venv().expect("test virtual environment should activate");
    }

    fn assert_every_element_resolves(
        result: &ExecutionResult,
        operation: SemanticOperation,
        line: u32,
    ) {
        let strategy = NullIdentification;
        for index in 0..result.ledger.face_count() {
            let context = resolve_context(
                &TopologyElement::Face(FaceId(index as u32)),
                &result.ledger,
                &strategy,
            )
            .unwrap();
            let entry = context.provenance.resolved().expect("resolved provenance");
            assert_eq!(entry.operation, operation);
            assert_eq!(entry.source.line, line);
        }
        for index in 0..result.ledger.edge_count() {
            let context = resolve_context(
                &TopologyElement::Edge(EdgeId(index as u32)),
                &result.ledger,
                &strategy,
            )
            .unwrap();
            let entry = context.provenance.resolved().expect("resolved provenance");
            assert_eq!(entry.operation, operation);
            assert_eq!(entry.source.line, line);
        }
        for index in 0..result.ledger.vertex_count() {
            let context = resolve_context(
                &TopologyElement::Vertex(VertexId(index as u32)),
                &result.ledger,
                &strategy,
            )
            .unwrap();
            let entry = context.provenance.resolved().expect("resolved provenance");
            assert_eq!(entry.operation, operation);
            assert_eq!(entry.source.line, line);
        }
    }

    fn resolved_contexts(result: &ExecutionResult) -> Vec<GeometryContext> {
        let strategy = NullIdentification;
        let mut contexts = Vec::new();
        for index in 0..result.ledger.face_count() {
            contexts.push(
                resolve_context(
                    &TopologyElement::Face(FaceId(index as u32)),
                    &result.ledger,
                    &strategy,
                )
                .unwrap(),
            );
        }
        for index in 0..result.ledger.edge_count() {
            contexts.push(
                resolve_context(
                    &TopologyElement::Edge(EdgeId(index as u32)),
                    &result.ledger,
                    &strategy,
                )
                .unwrap(),
            );
        }
        for index in 0..result.ledger.vertex_count() {
            contexts.push(
                resolve_context(
                    &TopologyElement::Vertex(VertexId(index as u32)),
                    &result.ledger,
                    &strategy,
                )
                .unwrap(),
            );
        }
        contexts
    }

    fn assert_bridge_consumers(result: &ExecutionResult) {
        let strategy = NullIdentification;
        let representative = [
            TopologyElement::Face(FaceId(0)),
            TopologyElement::Edge(EdgeId(0)),
            TopologyElement::Vertex(VertexId(0)),
        ];
        for element in representative {
            let context = resolve_context(&element, &result.ledger, &strategy).unwrap();
            let expected = context.provenance.clone();
            let request = cadmark_bridge::context::AiRequest::from_spatial_comment(
                "source".into(),
                "comment".into(),
                context,
            );
            let actual = request.geometry_context.unwrap().provenance;
            assert_eq!(actual, expected);
        }
    }

    fn assert_contains_operation(result: &ExecutionResult, operation: SemanticOperation) {
        assert!(
            resolved_contexts(result).iter().any(|context| {
                context
                    .provenance
                    .resolved()
                    .is_some_and(|entry| entry.operation == operation)
            }),
            "no final topology resolved to {operation:?}"
        );
    }

    /// The resolved entry of a context, for assertions on single-source elements.
    fn entry(context: &GeometryContext) -> &ProvenanceEntry {
        context.provenance.resolved().expect("resolved provenance")
    }

    #[test]
    fn real_box_resolves_all_final_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
"#,
        )
        .unwrap();

        assert_eq!(result.ledger.face_count(), 6);
        assert_eq!(result.ledger.edge_count(), 12);
        assert_eq!(result.ledger.vertex_count(), 8);
        assert_every_element_resolves(&result, SemanticOperation::Box, 4);
        assert!(matches!(
            result.ledger.lookup_face(FaceId(0)),
            Some(LedgerValue::Resolved(_))
        ));
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_cylinder_resolves_all_final_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Cylinder(5, 10)
"#,
        )
        .unwrap();
        assert_every_element_resolves(&result, SemanticOperation::Cylinder, 4);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_boolean_fuse_resolves_changed_and_unchanged_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    with Locations((5, 5, 5)):
        Box(10, 10, 10)
"#,
        )
        .unwrap();
        assert_contains_operation(&result, SemanticOperation::BooleanFuse);
        assert_contains_operation(&result, SemanticOperation::Box);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_boolean_cut_resolves_all_surviving_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    with Locations((2, 1, 0)):
        Box(4, 4, 14, mode=Mode.SUBTRACT)
"#,
        )
        .unwrap();
        assert_contains_operation(&result, SemanticOperation::BooleanCut);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_boolean_common_resolves_all_surviving_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    with Locations((5, 5, 5)):
        Box(10, 10, 10, mode=Mode.INTERSECT)
"#,
        )
        .unwrap();
        assert_contains_operation(&result, SemanticOperation::BooleanCommon);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_fillet_resolves_direct_and_descendant_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    fillet(part.edges().filter_by(Axis.Z), radius=1)
"#,
        )
        .unwrap();
        assert_contains_operation(&result, SemanticOperation::Fillet);
        let contexts = resolved_contexts(&result);
        assert!(contexts.iter().any(|context| {
            entry(context).operation == SemanticOperation::Fillet
                && matches!(context.element, TopologyElement::Face(_))
                && entry(context).relation == ProvenanceRelation::Generated
        }));
        assert!(contexts.iter().any(|context| {
            entry(context).operation == SemanticOperation::Fillet
                && entry(context).relation == ProvenanceRelation::Modified
        }));
        assert!(contexts.iter().any(|context| {
            entry(context).operation == SemanticOperation::Fillet
                && matches!(context.element, TopologyElement::Edge(_))
                && matches!(
                    entry(context).relation,
                    ProvenanceRelation::GeneratedDescendant
                        | ProvenanceRelation::ModifiedDescendant
                )
        }));
        assert!(contexts.iter().any(|context| {
            entry(context).operation == SemanticOperation::Fillet
                && matches!(context.element, TopologyElement::Vertex(_))
                && matches!(
                    entry(context).relation,
                    ProvenanceRelation::GeneratedDescendant
                        | ProvenanceRelation::ModifiedDescendant
                )
        }));
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_chamfer_resolves_direct_and_descendant_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    chamfer(part.edges().filter_by(Axis.Z), length=1)
"#,
        )
        .unwrap();
        assert_contains_operation(&result, SemanticOperation::Chamfer);
        let contexts = resolved_contexts(&result);
        assert!(contexts.iter().any(|context| {
            entry(context).operation == SemanticOperation::Chamfer
                && matches!(context.element, TopologyElement::Face(_))
                && entry(context).relation == ProvenanceRelation::Generated
        }));
        assert!(contexts.iter().any(|context| {
            entry(context).operation == SemanticOperation::Chamfer
                && entry(context).relation == ProvenanceRelation::Modified
        }));
        assert!(contexts.iter().any(|context| {
            entry(context).operation == SemanticOperation::Chamfer
                && matches!(context.element, TopologyElement::Edge(_))
                && matches!(
                    entry(context).relation,
                    ProvenanceRelation::GeneratedDescendant
                        | ProvenanceRelation::ModifiedDescendant
                )
        }));
        assert!(contexts.iter().any(|context| {
            entry(context).operation == SemanticOperation::Chamfer
                && matches!(context.element, TopologyElement::Vertex(_))
                && matches!(
                    entry(context).relation,
                    ProvenanceRelation::GeneratedDescendant
                        | ProvenanceRelation::ModifiedDescendant
                )
        }));
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_locations_transport_primitive_provenance() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    with Locations((0, 0, 0), (10, 0, 0)):
        Box(2, 2, 2)
"#,
        )
        .unwrap();
        assert_eq!(result.ledger.face_count(), 12);
        assert_eq!(result.ledger.edge_count(), 24);
        assert_eq!(result.ledger.vertex_count(), 16);
        assert_every_element_resolves(&result, SemanticOperation::Box, 5);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_grid_locations_transport_primitive_provenance() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    with GridLocations(20, 20, 2, 2):
        Box(2, 2, 2)
"#,
        )
        .unwrap();
        assert_eq!(result.ledger.face_count(), 24);
        assert_eq!(result.ledger.edge_count(), 48);
        assert_eq!(result.ledger.vertex_count(), 32);
        assert_every_element_resolves(&result, SemanticOperation::Box, 5);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_polar_locations_transport_rotated_primitive_provenance() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    with PolarLocations(10, 3):
        Box(2, 2, 2)
"#,
        )
        .unwrap();
        assert_eq!(result.ledger.face_count(), 18);
        assert_eq!(result.ledger.edge_count(), 36);
        assert_eq!(result.ledger.vertex_count(), 24);
        assert_every_element_resolves(&result, SemanticOperation::Box, 5);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_sketch_extrude_resolves_all_final_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    with BuildSketch():
        Rectangle(20, 10)
    extrude(amount=5)
"#,
        )
        .unwrap();
        assert_eq!(result.ledger.face_count(), 6);
        assert_every_element_resolves(&result, SemanticOperation::Extrude, 6);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_sketch_cut_extrude_through_box_resolves_all_surviving_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

hole_diameter_mm = 3.4
hole_inset_mm = 5

with BuildPart() as part:
    Box(40, 25, 12)
    top_face = part.faces().sort_by(Axis.Z)[-1]
    with BuildSketch(top_face):
        with GridLocations(30, 15, 2, 2):
            Circle(hole_diameter_mm / 2)
    extrude(amount=-12, mode=Mode.SUBTRACT)
"#,
        )
        .unwrap();
        assert_eq!(result.ledger.face_count(), 10);
        assert_contains_operation(&result, SemanticOperation::BooleanCut);
        assert_contains_operation(&result, SemanticOperation::Box);
        let contexts = resolved_contexts(&result);
        assert!(contexts.iter().any(|context| {
            entry(context).operation == SemanticOperation::Extrude
                && matches!(context.element, TopologyElement::Face(_))
                && entry(context).source.line == 12
        }));
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_revolve_resolves_all_final_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    with BuildSketch(Plane.XZ) as profile:
        with Locations((6, 0)):
            Rectangle(4, 4)
    revolve(axis=Axis.Z)
"#,
        )
        .unwrap();
        assert_every_element_resolves(&result, SemanticOperation::Revolve, 7);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_sphere_cone_torus_and_wedge_resolve_all_final_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Sphere(5)
    with Locations((20, 0, 0)):
        Cone(4, 2, 8)
    with Locations((0, 20, 0)):
        Torus(4, 1)
    with Locations((0, -20, 0)):
        Wedge(4, 4, 4, 1, 1, 3, 3)
"#,
        )
        .unwrap();
        assert_eq!(result.ledger.untraced_count(), 0);
        for operation in [
            SemanticOperation::Sphere,
            SemanticOperation::Cone,
            SemanticOperation::Torus,
            SemanticOperation::Wedge,
        ] {
            assert_contains_operation(&result, operation);
        }
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_loft_and_sweep_resolve_all_final_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    with BuildSketch(Plane.XY) as base:
        Rectangle(6, 6)
    with BuildSketch(Plane.XY.offset(8)) as top:
        Circle(2)
    loft()
    with BuildLine() as path:
        Polyline((20, 0, 0), (20, 0, 10), (25, 0, 15))
    with BuildSketch(Plane.XZ):
        with Locations((20, 0)):
            Circle(1)
    sweep(path=path.line)
"#,
        )
        .unwrap();
        assert_eq!(result.ledger.untraced_count(), 0);
        assert_contains_operation(&result, SemanticOperation::Loft);
        assert_contains_operation(&result, SemanticOperation::Sweep);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_shell_mirror_split_and_taper_resolve_all_final_topology() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Box(20, 20, 10)
    offset(amount=-2, openings=part.faces().sort_by(Axis.Z)[-1])
    mirror(about=Plane.YZ)
    split(bisect_by=Plane.XZ, keep=Keep.TOP)
    with BuildSketch(part.faces().sort_by(Axis.Y)[-1]):
        Circle(3)
    extrude(amount=4, taper=10)
"#,
        )
        .unwrap();
        assert_eq!(result.ledger.untraced_count(), 0);
        assert_contains_operation(&result, SemanticOperation::Split);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn untraced_geometry_still_renders_and_reports_untraced_on_selection() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *
from OCP.BRepPrimAPI import BRepPrimAPI_MakeSphere

with BuildPart() as part:
    add(Solid(BRepPrimAPI_MakeSphere(5).Shape()))
"#,
        )
        .unwrap();
        assert!(!result.mesh.indices.is_empty());
        assert_eq!(result.ledger.face_count(), 1);
        assert_eq!(result.ledger.untraced_count(), result.ledger.len());
        let context = resolve_context(
            &TopologyElement::Face(FaceId(0)),
            &result.ledger,
            &NullIdentification,
        )
        .unwrap();
        assert_eq!(context.provenance, LedgerValue::Untraced);
    }

    #[test]
    fn execution_measures_every_final_element_and_the_whole_model() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Box(20, 10, 5)
"#,
        )
        .unwrap();
        assert_eq!(result.descriptors.faces.len(), result.ledger.face_count());
        assert_eq!(result.descriptors.edges.len(), result.ledger.edge_count());
        assert_eq!(
            result.descriptors.vertices.len(),
            result.ledger.vertex_count()
        );
        assert!(
            result
                .descriptors
                .faces
                .iter()
                .all(|face| face.surface_type == "plane")
        );
        let top = result
            .descriptors
            .faces
            .iter()
            .find(|face| face.normal[2] > 0.99)
            .expect("box has an upward face");
        assert!((top.area - 200.0).abs() < 1e-6);
        assert!((top.centre[2] - 2.5).abs() < 1e-6);
        assert!((result.summary.volume - 1000.0).abs() < 1e-6);
        let size = result.summary.size();
        assert!((size[0] - 20.0).abs() < 1e-3);
        assert!((size[1] - 10.0).abs() < 1e-3);
        assert!((size[2] - 5.0).abs() < 1e-3);
        assert_eq!(result.summary.face_count, 6);
    }

    #[test]
    fn model_bounds_are_exact_for_curved_geometry() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Cylinder(10, 5)
"#,
        )
        .unwrap();
        let size = result.summary.size();
        assert!((size[0] - 20.0).abs() < 1e-6, "{size:?}");
        assert!((size[1] - 20.0).abs() < 1e-6, "{size:?}");
        assert!((size[2] - 5.0).abs() < 1e-6, "{size:?}");
    }

    #[test]
    fn tessellation_carries_outward_surface_normals() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Cylinder(5, 10)
"#,
        )
        .unwrap();
        let outward = result
            .mesh
            .vertices
            .iter()
            .filter(|vertex| vertex.normal[2].abs() < 0.01)
            .filter(|vertex| {
                let radial = [vertex.position[0], vertex.position[1]];
                let dot = radial[0] * vertex.normal[0] + radial[1] * vertex.normal[1];
                dot > 0.0
            })
            .count();
        let side = result
            .mesh
            .vertices
            .iter()
            .filter(|vertex| vertex.normal[2].abs() < 0.01)
            .count();
        assert!(side > 0, "cylinder wall vertices carry sideways normals");
        assert_eq!(outward, side, "every wall normal points away from the axis");
    }

    #[test]
    fn exports_step_stl_and_3mf_from_the_executed_model() {
        activate_test_runtime();
        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Box(20, 10, 5)
"#,
        )
        .unwrap();
        let directory = std::env::temp_dir().join(format!(
            "cadmark-export-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::create_dir_all(&directory).unwrap();
        for format in [
            cadmark_core::export::ExportFormat::Step,
            cadmark_core::export::ExportFormat::Stl,
            cadmark_core::export::ExportFormat::ThreeMf,
        ] {
            let path = directory.join(format!("part.{}", format.extension()));
            crate::export::export_model(&result.model, format, &path).unwrap();
            assert!(std::fs::metadata(&path).unwrap().len() > 0, "{path:?}");
        }
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn execution_error_restores_bindings_for_the_next_run() {
        activate_test_runtime();
        let error = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Box(1, 1, 1)
raise RuntimeError("deliberate execution failure")
"#,
        )
        .unwrap_err();
        let ExecutionError::Script(message) = error else {
            panic!("expected a script failure, got {error:?}");
        };
        assert!(message.contains("line 5"));
        assert!(message.contains("deliberate execution failure"));
        assert!(message.contains("RuntimeError"));

        let result = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Cylinder(2, 4)
"#,
        )
        .unwrap();
        assert_every_element_resolves(&result, SemanticOperation::Cylinder, 4);
    }

    #[test]
    fn file_execution_preserves_the_real_filename_and_source_line() {
        activate_test_runtime();
        let path = std::env::temp_dir().join(format!(
            "cadmark-provenance-{}-{}.py",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(
            &path,
            r#"from build123d import *

with BuildPart() as part:
    Box(3, 4, 5)
"#,
        )
        .unwrap();
        let result = execute_script(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_every_element_resolves(&result, SemanticOperation::Box, 4);
    }

    #[test]
    fn identity_implementation_contains_no_shape_hash_fallback() {
        let instrumentation = include_str!("provenance_instrumentation.py");
        let tessellation = include_str!("tessellation.rs");
        assert!(!instrumentation.contains("HashCode"));
        assert!(!instrumentation.contains("hash("));
        assert!(!tessellation.contains("HashCode"));
        assert!(!tessellation.contains("shape_hash"));
    }

    #[test]
    fn tessellation_ids_outside_final_maps_are_rejected() {
        let entry = LedgerValue::Resolved(ProvenanceEntry {
            source: SourceRef {
                line: 1,
                code: "Box(1, 1, 1)".into(),
            },
            operation: SemanticOperation::Box,
            operation_id: 1,
            relation: ProvenanceRelation::Generated,
        });
        let mut ledger = ProvenanceLedger::new();
        ledger.record_face(FaceId(0), entry.clone()).unwrap();
        ledger.record_edge(EdgeId(0), entry).unwrap();
        let mesh = TessellatedMesh {
            vertices: Vec::new(),
            indices: vec![0, 1, 2],
            face_ids: vec![1],
            edges: vec![crate::tessellation::MeshEdge {
                points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                edge_id: 1,
            }],
        };
        assert!(matches!(
            validate_tessellation_ids(&mesh, &ledger),
            Err(ExecutionError::Tessellation(
                crate::tessellation::TessellationError::InvalidTopologyId { .. }
            ))
        ));
    }
}
