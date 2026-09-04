// Script execution — runs a build123d script in the embedded interpreter and
// captures everything the application keeps: mesh, provenance, measurements,
// validity, and the model itself as a file.
//
// This runs inside the confined kernel worker process (see `worker.rs`);
// the application never calls it directly.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once};

use cadmark_core::ledger::ProvenanceLedger;
use cadmark_core::mesh::TessellatedMesh;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use thiserror::Error;

use crate::protocol::{ExecutedModel, ModelFile};

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
    #[error("Could not keep the model for export: {0}")]
    ModelFile(String),
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

/// Locate the project virtual environment, in this order:
/// 1. `VIRTUAL_ENV` in the environment
/// 2. `.venv/` in the workspace root (baked in at compile time)
/// 3. `.venv/` near the running executable (walk up 4 levels)
/// 4. `.venv/` in the current working directory
pub fn discover_venv() -> Option<PathBuf> {
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        let path = PathBuf::from(venv);
        if path.is_dir() {
            return Some(path);
        }
    }

    // Compile-time workspace root: CARGO_MANIFEST_DIR points at
    // crates/cadmark-kernel/, so the workspace root is two levels up.
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace_root) = manifest_dir.parent().and_then(|p| p.parent()) {
        let candidate = workspace_root.join(".venv");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        for ancestor in exe_dir.ancestors().take(4) {
            let candidate = ancestor.join(".venv");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join(".venv");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    None
}

/// Discover and activate the project venv. Without one, build123d is not
/// importable and every script fails at its first line.
pub fn discover_and_activate_venv() -> Result<(), ExecutionError> {
    crate::python_runtime::configure_python_home();
    match discover_venv() {
        Some(venv) => activate_venv(&venv),
        None => {
            log::warn!("No Python venv found — build123d may not be importable");
            Ok(())
        }
    }
}

/// Execute a build123d script and return everything the application keeps.
/// The model is written as a BREP file into `scratch_dir`, which the caller
/// owns.
///
/// This function acquires the GIL and must not be called from the
/// UI or rendering thread.
pub fn execute_script(
    script_path: &Path,
    scratch_dir: &Path,
) -> Result<ExecutedModel, ExecutionError> {
    if !script_path.exists() {
        return Err(ExecutionError::ScriptNotFound(
            script_path.display().to_string(),
        ));
    }

    let script_content = std::fs::read_to_string(script_path)
        .map_err(|e| ExecutionError::ScriptNotFound(e.to_string()))?;

    execute_script_source_named(
        &script_content,
        &script_path.display().to_string(),
        scratch_dir,
    )
}

/// Execute build123d source code directly (for tests).
pub fn execute_script_source(
    source: &str,
    scratch_dir: &Path,
) -> Result<ExecutedModel, ExecutionError> {
    execute_script_source_named(source, "<cadmark-source>", scratch_dir)
}

/// Every execution's model file gets a fresh name so an export of the
/// previous model can never read a half-written successor.
static MODEL_FILE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn execute_script_source_named(
    source: &str,
    filename: &str,
    scratch_dir: &Path,
) -> Result<ExecutedModel, ExecutionError> {
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
            let validity = crate::measurement::solid_validity(py, &ocp_shape)?;
            let model = keep_model(py, &shape, scratch_dir)?;
            Ok(ExecutedModel {
                mesh,
                ledger,
                descriptors,
                summary,
                validity,
                model,
            })
        })();

        let restoration = crate::provenance::restore(py, &session);
        if let Err(error) = restoration {
            return Err(ExecutionError::Provenance(error));
        }
        execution
    })
}

/// Write the executed model to a fresh BREP file in `scratch_dir` so it can
/// be exported later without re-running the script.
fn keep_model(
    py: Python<'_>,
    shape: &Bound<'_, PyAny>,
    scratch_dir: &Path,
) -> Result<ModelFile, ExecutionError> {
    let sequence = MODEL_FILE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = scratch_dir.join(format!("model-{sequence}.brep"));
    let exporters = py.import("build123d.exporters3d")?;
    let written: bool = exporters
        .call_method1("export_brep", (shape, path.display().to_string()))?
        .extract()?;
    if !written {
        return Err(ExecutionError::ModelFile(path.display().to_string()));
    }
    Ok(ModelFile(path))
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

    /// Execute source with a scratch directory that lives as long as the
    /// returned guard.
    fn run(source: &str) -> (tempfile::TempDir, Result<ExecutedModel, ExecutionError>) {
        activate_test_runtime();
        let scratch = tempfile::tempdir().unwrap();
        let result = execute_script_source(source, scratch.path());
        (scratch, result)
    }

    fn assert_every_element_resolves(
        result: &ExecutedModel,
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

    fn resolved_contexts(result: &ExecutedModel) -> Vec<GeometryContext> {
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

    /// The contexts the bridge renders for the model round-trip through
    /// the grounding text: every element's provenance reaches it.
    fn assert_bridge_consumers(result: &ExecutedModel) {
        let strategy = NullIdentification;
        let representative = [
            TopologyElement::Face(FaceId(0)),
            TopologyElement::Edge(EdgeId(0)),
            TopologyElement::Vertex(VertexId(0)),
        ];
        for element in representative {
            let context = resolve_context(&element, &result.ledger, &strategy).unwrap();
            let label = element.display_label();
            let rendered = cadmark_bridge::grounding::render_comment(
                &cadmark_bridge::grounding::GroundedComment {
                    text: "comment".into(),
                    anchors: vec![context.clone()],
                },
            );
            assert!(rendered.contains(&label), "{rendered}");
            if let Some(entry) = context.provenance.resolved() {
                assert!(rendered.contains(&format!("line {}", entry.source.line)));
            }
        }
    }

    fn assert_contains_operation(result: &ExecutedModel, operation: SemanticOperation) {
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
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
"#,
        );
        let result = result.unwrap();

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
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Cylinder(5, 10)
"#,
        );
        let result = result.unwrap();
        assert_every_element_resolves(&result, SemanticOperation::Cylinder, 4);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_boolean_fuse_resolves_changed_and_unchanged_topology() {
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    with Locations((5, 5, 5)):
        Box(10, 10, 10)
"#,
        );
        let result = result.unwrap();
        assert_contains_operation(&result, SemanticOperation::BooleanFuse);
        assert_contains_operation(&result, SemanticOperation::Box);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_boolean_cut_resolves_all_surviving_topology() {
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    with Locations((2, 1, 0)):
        Box(4, 4, 14, mode=Mode.SUBTRACT)
"#,
        );
        let result = result.unwrap();
        assert_contains_operation(&result, SemanticOperation::BooleanCut);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_boolean_common_resolves_all_surviving_topology() {
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    with Locations((5, 5, 5)):
        Box(10, 10, 10, mode=Mode.INTERSECT)
"#,
        );
        let result = result.unwrap();
        assert_contains_operation(&result, SemanticOperation::BooleanCommon);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_fillet_resolves_direct_and_descendant_topology() {
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    fillet(part.edges().filter_by(Axis.Z), radius=1)
"#,
        );
        let result = result.unwrap();
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
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    chamfer(part.edges().filter_by(Axis.Z), length=1)
"#,
        );
        let result = result.unwrap();
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
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    with Locations((0, 0, 0), (10, 0, 0)):
        Box(2, 2, 2)
"#,
        );
        let result = result.unwrap();
        assert_eq!(result.ledger.face_count(), 12);
        assert_eq!(result.ledger.edge_count(), 24);
        assert_eq!(result.ledger.vertex_count(), 16);
        assert_every_element_resolves(&result, SemanticOperation::Box, 5);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_grid_locations_transport_primitive_provenance() {
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    with GridLocations(20, 20, 2, 2):
        Box(2, 2, 2)
"#,
        );
        let result = result.unwrap();
        assert_eq!(result.ledger.face_count(), 24);
        assert_eq!(result.ledger.edge_count(), 48);
        assert_eq!(result.ledger.vertex_count(), 32);
        assert_every_element_resolves(&result, SemanticOperation::Box, 5);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_polar_locations_transport_rotated_primitive_provenance() {
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    with PolarLocations(10, 3):
        Box(2, 2, 2)
"#,
        );
        let result = result.unwrap();
        assert_eq!(result.ledger.face_count(), 18);
        assert_eq!(result.ledger.edge_count(), 36);
        assert_eq!(result.ledger.vertex_count(), 24);
        assert_every_element_resolves(&result, SemanticOperation::Box, 5);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_sketch_extrude_resolves_all_final_topology() {
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    with BuildSketch():
        Rectangle(20, 10)
    extrude(amount=5)
"#,
        );
        let result = result.unwrap();
        assert_eq!(result.ledger.face_count(), 6);
        assert_every_element_resolves(&result, SemanticOperation::Extrude, 6);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_sketch_cut_extrude_through_box_resolves_all_surviving_topology() {
        let (_scratch, result) = run(
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
        );
        let result = result.unwrap();
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
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    with BuildSketch(Plane.XZ) as profile:
        with Locations((6, 0)):
            Rectangle(4, 4)
    revolve(axis=Axis.Z)
"#,
        );
        let result = result.unwrap();
        assert_every_element_resolves(&result, SemanticOperation::Revolve, 7);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_sphere_cone_torus_and_wedge_resolve_all_final_topology() {
        let (_scratch, result) = run(
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
        );
        let result = result.unwrap();
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
        let (_scratch, result) = run(
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
        );
        let result = result.unwrap();
        assert_eq!(result.ledger.untraced_count(), 0);
        assert_contains_operation(&result, SemanticOperation::Loft);
        assert_contains_operation(&result, SemanticOperation::Sweep);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn real_shell_mirror_split_and_taper_resolve_all_final_topology() {
        let (_scratch, result) = run(
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
        );
        let result = result.unwrap();
        assert_eq!(result.ledger.untraced_count(), 0);
        assert_contains_operation(&result, SemanticOperation::Split);
        assert_bridge_consumers(&result);
    }

    #[test]
    fn untraced_geometry_still_renders_and_reports_untraced_on_selection() {
        let (_scratch, result) = run(
            r#"from build123d import *
from OCP.BRepPrimAPI import BRepPrimAPI_MakeSphere

with BuildPart() as part:
    add(Solid(BRepPrimAPI_MakeSphere(5).Shape()))
"#,
        );
        let result = result.unwrap();
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
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(20, 10, 5)
"#,
        );
        let result = result.unwrap();
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
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Cylinder(10, 5)
"#,
        );
        let result = result.unwrap();
        let size = result.summary.size();
        assert!((size[0] - 20.0).abs() < 1e-6, "{size:?}");
        assert!((size[1] - 20.0).abs() < 1e-6, "{size:?}");
        assert!((size[2] - 5.0).abs() < 1e-6, "{size:?}");
    }

    /// Every triangle of a closed solid faces outward: its normals point away
    /// from the centre and its winding is anticlockwise seen from outside.
    /// A box exercises both face orientations, since half its faces are
    /// reversed relative to their planes.
    #[test]
    fn tessellation_faces_outward_on_reversed_faces() {
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 20, 30)
"#,
        );
        let result = result.unwrap();
        let mesh = &result.mesh;
        for vertex in &mesh.vertices {
            let dot: f32 = (0..3)
                .map(|axis| vertex.position[axis] * vertex.normal[axis])
                .sum();
            assert!(dot > 0.0, "normal points inward at {vertex:?}");
        }
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = [
                mesh.vertices[triangle[0] as usize].position,
                mesh.vertices[triangle[1] as usize].position,
                mesh.vertices[triangle[2] as usize].position,
            ];
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let winding_normal = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            let outward: f32 = (0..3).map(|axis| winding_normal[axis] * a[axis]).sum();
            assert!(
                outward > 0.0,
                "triangle {triangle:?} is wound clockwise from outside"
            );
        }
    }

    #[test]
    fn tessellation_carries_outward_surface_normals() {
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Cylinder(5, 10)
"#,
        );
        let result = result.unwrap();
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
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(20, 10, 5)
"#,
        );
        let result = result.unwrap();
        assert!(result.model.0.is_file(), "model kept at {:?}", result.model);
        let directory = tempfile::tempdir().unwrap();
        for format in cadmark_core::export::ExportFormat::ALL {
            let path = directory.path().join(format!("part.{}", format.extension()));
            crate::export::export_model(&result.model, format, &path).unwrap();
            assert!(std::fs::metadata(&path).unwrap().len() > 0, "{path:?}");
        }
    }

    #[test]
    fn algebra_mode_and_direct_api_scripts_execute_with_provenance() {
        let (_scratch, algebra) = run(
            r#"from build123d import *

plate = Box(20, 10, 5)
hole = Cylinder(2, 10)
result = plate - hole
"#,
        );
        let algebra = algebra.unwrap();
        assert!(algebra.summary.face_count > 6, "the hole adds faces");
        assert_contains_operation(&algebra, SemanticOperation::BooleanCut);
        assert!(algebra.is_printable());

        let (_scratch, direct) = run(
            r#"from build123d import *

block = Solid.make_box(4, 4, 4)
"#,
        );
        let direct = direct.unwrap();
        assert_eq!(direct.ledger.face_count(), 6);
        assert_every_element_resolves(&direct, SemanticOperation::Box, 3);
    }

    #[test]
    fn a_closed_solid_is_printable_and_an_open_shell_is_flagged() {
        let (_scratch, closed) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
"#,
        );
        let closed = closed.unwrap();
        assert_eq!(closed.validity.len(), 1);
        assert!(closed.is_printable());

        // A box with one face removed, closed into a "solid" with a hole in
        // it: the shape the slicer would reject.
        let (_scratch, open) = run(
            r#"from build123d import *
from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeSolid
from OCP.TopoDS import TopoDS

box = Solid.make_box(10, 10, 10)
shell = Shell(box.faces()[:-1])
leaky = Solid(BRepBuilderAPI_MakeSolid(TopoDS.Shell_s(shell.wrapped)).Solid())
with BuildPart() as part:
    add(leaky)
"#,
        );
        let open = open.unwrap();
        assert_eq!(open.validity.len(), 1);
        assert!(!open.validity[0].closed);
        assert!(!open.is_printable());
    }

    #[test]
    fn execution_error_restores_bindings_for_the_next_run() {
        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Box(1, 1, 1)
raise RuntimeError("deliberate execution failure")
"#,
        );
        let error = result.unwrap_err();
        let ExecutionError::Script(message) = error else {
            panic!("expected a script failure, got {error:?}");
        };
        assert!(message.contains("line 5"));
        assert!(message.contains("deliberate execution failure"));
        assert!(message.contains("RuntimeError"));

        let (_scratch, result) = run(
            r#"from build123d import *

with BuildPart() as part:
    Cylinder(2, 4)
"#,
        );
        assert_every_element_resolves(&result.unwrap(), SemanticOperation::Cylinder, 4);
    }

    #[test]
    fn file_execution_preserves_the_real_filename_and_source_line() {
        activate_test_runtime();
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("part.py");
        std::fs::write(
            &path,
            r#"from build123d import *

with BuildPart() as part:
    Box(3, 4, 5)
"#,
        )
        .unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let result = execute_script(&path, scratch.path()).unwrap();
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
            edges: vec![cadmark_core::mesh::MeshEdge {
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
