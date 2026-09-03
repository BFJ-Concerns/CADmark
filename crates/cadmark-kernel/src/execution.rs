// Script execution — runs build123d Python scripts and captures results.

use std::path::Path;
use std::sync::{Mutex, Once};

use cadmark_core::ledger::ProvenanceLedger;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use thiserror::Error;

use crate::tessellation::TessellatedMesh;

#[derive(Error, Debug)]
pub enum ExecutionError {
    #[error("Python error: {0}")]
    Python(#[from] PyErr),
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
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            for ancestor in exe_dir.ancestors().take(4) {
                let candidate = ancestor.join(".venv");
                if candidate.is_dir() {
                    return activate_venv(&candidate);
                }
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
        if log::log_enabled!(log::Level::Debug) {
            if let Ok(sys) = py.import("sys") {
                if let Ok(version) = sys.getattr("version") {
                    log::debug!("Python version: {version}");
                }
                if let Ok(path) = sys.getattr("path") {
                    log::debug!("sys.path: {path}");
                }
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
                    log::error!("Python execution failed: {error}");
                    ExecutionError::Python(error)
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
            Ok(ExecutionResult { mesh, ledger })
        })();

        let restoration = crate::provenance::restore(py, &session);
        if let Err(error) = restoration {
            return Err(ExecutionError::Provenance(error));
        }
        execution
    })
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
            assert_eq!(context.provenance.operation, operation);
            assert_eq!(context.provenance.source.line, line);
        }
        for index in 0..result.ledger.edge_count() {
            let context = resolve_context(
                &TopologyElement::Edge(EdgeId(index as u32)),
                &result.ledger,
                &strategy,
            )
            .unwrap();
            assert_eq!(context.provenance.operation, operation);
            assert_eq!(context.provenance.source.line, line);
        }
        for index in 0..result.ledger.vertex_count() {
            let context = resolve_context(
                &TopologyElement::Vertex(VertexId(index as u32)),
                &result.ledger,
                &strategy,
            )
            .unwrap();
            assert_eq!(context.provenance.operation, operation);
            assert_eq!(context.provenance.source.line, line);
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
            resolved_contexts(result)
                .iter()
                .any(|context| context.provenance.operation == operation),
            "no final topology resolved to {operation:?}"
        );
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
            context.provenance.operation == SemanticOperation::Fillet
                && matches!(context.element, TopologyElement::Face(_))
                && context.provenance.relation == ProvenanceRelation::Generated
        }));
        assert!(contexts.iter().any(|context| {
            context.provenance.operation == SemanticOperation::Fillet
                && context.provenance.relation == ProvenanceRelation::Modified
        }));
        assert!(contexts.iter().any(|context| {
            context.provenance.operation == SemanticOperation::Fillet
                && matches!(context.element, TopologyElement::Edge(_))
                && matches!(
                    context.provenance.relation,
                    ProvenanceRelation::GeneratedDescendant
                        | ProvenanceRelation::ModifiedDescendant
                )
        }));
        assert!(contexts.iter().any(|context| {
            context.provenance.operation == SemanticOperation::Fillet
                && matches!(context.element, TopologyElement::Vertex(_))
                && matches!(
                    context.provenance.relation,
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
            context.provenance.operation == SemanticOperation::Chamfer
                && matches!(context.element, TopologyElement::Face(_))
                && context.provenance.relation == ProvenanceRelation::Generated
        }));
        assert!(contexts.iter().any(|context| {
            context.provenance.operation == SemanticOperation::Chamfer
                && context.provenance.relation == ProvenanceRelation::Modified
        }));
        assert!(contexts.iter().any(|context| {
            context.provenance.operation == SemanticOperation::Chamfer
                && matches!(context.element, TopologyElement::Edge(_))
                && matches!(
                    context.provenance.relation,
                    ProvenanceRelation::GeneratedDescendant
                        | ProvenanceRelation::ModifiedDescendant
                )
        }));
        assert!(contexts.iter().any(|context| {
            context.provenance.operation == SemanticOperation::Chamfer
                && matches!(context.element, TopologyElement::Vertex(_))
                && matches!(
                    context.provenance.relation,
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
            context.provenance.operation == SemanticOperation::Extrude
                && matches!(context.element, TopologyElement::Face(_))
                && context.provenance.source.line == 12
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
    fn unsupported_sphere_fails_with_missing_history() {
        activate_test_runtime();
        let error = execute_script_source(
            r#"from build123d import *

with BuildPart() as part:
    Sphere(5)
"#,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ExecutionError::Provenance(crate::provenance::ProvenanceError::MissingHistory { .. })
        ));
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
        assert!(matches!(error, ExecutionError::Python(_)));

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
