//! Version-bound OCP instrumentation and provenance schema validation.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};

use cadmark_core::geometry::{EdgeId, FaceId, VertexId};
use cadmark_core::ledger::{
    LedgerValue, ProvenanceEntry, ProvenanceLedger, ProvenanceRelation, SemanticOperation,
    SourceRef,
};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use thiserror::Error;

const INSTRUMENTATION_SOURCE: &str = include_str!("provenance_instrumentation.py");
static INSTRUMENTATION_POISONED: AtomicBool = AtomicBool::new(false);

#[derive(Error, Debug)]
pub enum ProvenanceError {
    #[error("Python error during provenance capture: {0}")]
    Python(#[from] PyErr),
    #[error("unsupported provenance runtime: {0}")]
    UnsupportedRuntimeVersion(String),
    #[error("failed to install provenance wrapper: {0}")]
    WrapperApplication(String),
    #[error("failed to restore provenance wrappers: {0}")]
    WrapperRestoration(String),
    #[error("provenance instrumentation is poisoned after a restoration failure")]
    Poisoned,
    #[error("malformed provenance capture: {0}")]
    MalformedCapture(String),
}

#[derive(Debug)]
pub(crate) struct InstrumentationSession {
    inner: Py<PyAny>,
}

impl InstrumentationSession {
    pub(crate) fn bound<'py>(&self, py: Python<'py>) -> &Bound<'py, PyAny> {
        self.inner.bind(py)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawOperation {
    pub operation_id: u64,
    pub source_line: u32,
    pub operation: SemanticOperation,
    pub api_class: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawCandidate {
    pub operation_id: u64,
    pub relation: ProvenanceRelation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawElement {
    pub candidates: Vec<RawCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionTombstone {
    pub operation_id: u64,
    pub kind: String,
    pub ordinal: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawProvenance {
    pub schema_version: u32,
    pub operations: Vec<RawOperation>,
    pub faces: Vec<RawElement>,
    pub edges: Vec<RawElement>,
    pub vertices: Vec<RawElement>,
    pub tombstones: Vec<DeletionTombstone>,
}

pub(crate) fn install_instrumentation(
    py: Python<'_>,
    namespace: &Bound<'_, PyDict>,
    filename: &str,
) -> Result<InstrumentationSession, ProvenanceError> {
    if INSTRUMENTATION_POISONED.load(Ordering::Acquire) {
        return Err(ProvenanceError::Poisoned);
    }

    let source = CString::new(INSTRUMENTATION_SOURCE)
        .map_err(|error| ProvenanceError::WrapperApplication(error.to_string()))?;
    py.run(&source, Some(namespace), None)
        .map_err(classify_install_error)?;
    let install = namespace.get_item("_cadmark_install")?.ok_or_else(|| {
        ProvenanceError::WrapperApplication("_cadmark_install is missing".to_string())
    })?;
    let session = install
        .call1((filename,))
        .map_err(classify_install_error)?
        .unbind();
    namespace.set_item("_cadmark_session", &session)?;
    Ok(InstrumentationSession { inner: session })
}

fn classify_install_error(error: PyErr) -> ProvenanceError {
    Python::with_gil(|py| {
        let type_name = error
            .get_type(py)
            .name()
            .map(|name| name.to_string())
            .unwrap_or_default();
        let message = error.to_string();
        match type_name.as_str() {
            "UnsupportedRuntimeVersion" => ProvenanceError::UnsupportedRuntimeVersion(message),
            "ProvenanceWrapperError" => ProvenanceError::WrapperApplication(message),
            _ => ProvenanceError::Python(error),
        }
    })
}

pub(crate) fn finalise(
    py: Python<'_>,
    session: &InstrumentationSession,
    shape: &Bound<'_, PyAny>,
    source: &str,
) -> Result<(RawProvenance, ProvenanceLedger), ProvenanceError> {
    let capture = session.inner.bind(py).call_method1("finalise", (shape,))?;
    let raw = parse_capture(&capture)?;
    let ledger = build_ledger(&raw, source)?;
    Ok((raw, ledger))
}

pub(crate) fn restore(
    py: Python<'_>,
    session: &InstrumentationSession,
) -> Result<(), ProvenanceError> {
    match session.inner.bind(py).call_method0("restore") {
        Ok(_) => Ok(()),
        Err(error) => {
            INSTRUMENTATION_POISONED.store(true, Ordering::Release);
            Err(ProvenanceError::WrapperRestoration(error.to_string()))
        }
    }
}

fn parse_capture(value: &Bound<'_, PyAny>) -> Result<RawProvenance, ProvenanceError> {
    let schema_version: u32 = item(value, "schema_version")?.extract()?;
    if schema_version != 1 {
        return Err(ProvenanceError::MalformedCapture(format!(
            "unknown schema version {schema_version}"
        )));
    }

    let operations = parse_operations(&item(value, "operations")?)?;
    let operation_ids: HashSet<u64> = operations
        .iter()
        .map(|operation| operation.operation_id)
        .collect();
    if operation_ids.len() != operations.len() {
        return Err(ProvenanceError::MalformedCapture(
            "duplicate declared operation ID".to_string(),
        ));
    }

    let faces = parse_elements(&item(value, "faces")?, &operation_ids)?;
    let edges = parse_elements(&item(value, "edges")?, &operation_ids)?;
    let vertices = parse_elements(&item(value, "vertices")?, &operation_ids)?;
    let tombstones = parse_tombstones(&item(value, "tombstones")?, &operation_ids)?;

    Ok(RawProvenance {
        schema_version,
        operations,
        faces,
        edges,
        vertices,
        tombstones,
    })
}

fn parse_operations(value: &Bound<'_, PyAny>) -> Result<Vec<RawOperation>, ProvenanceError> {
    let mut operations = Vec::new();
    for value in value.try_iter()? {
        let value = value?;
        operations.push(RawOperation {
            operation_id: item(&value, "operation_id")?.extract()?,
            source_line: item(&value, "source_line")?.extract()?,
            operation: parse_operation(&item(&value, "operation")?.extract::<String>()?)?,
            api_class: item(&value, "api_class")?.extract()?,
        });
    }
    Ok(operations)
}

fn parse_elements(
    value: &Bound<'_, PyAny>,
    operation_ids: &HashSet<u64>,
) -> Result<Vec<RawElement>, ProvenanceError> {
    let mut elements = Vec::new();
    for element in value.try_iter()? {
        let element = element?;
        let mut candidates = Vec::new();
        let mut unique = HashSet::new();
        for candidate in item(&element, "candidates")?.try_iter()? {
            let candidate = candidate?;
            let operation_id: u64 = item(&candidate, "operation_id")?.extract()?;
            if !operation_ids.contains(&operation_id) {
                return Err(ProvenanceError::MalformedCapture(format!(
                    "candidate references unknown operation {operation_id}"
                )));
            }
            let relation = parse_relation(&item(&candidate, "relation")?.extract::<String>()?)?;
            let candidate = RawCandidate {
                operation_id,
                relation,
            };
            if !unique.insert(candidate.clone()) {
                return Err(ProvenanceError::MalformedCapture(
                    "duplicate candidate".to_string(),
                ));
            }
            candidates.push(candidate);
        }
        elements.push(RawElement { candidates });
    }
    Ok(elements)
}

fn parse_tombstones(
    value: &Bound<'_, PyAny>,
    operation_ids: &HashSet<u64>,
) -> Result<Vec<DeletionTombstone>, ProvenanceError> {
    let mut tombstones = Vec::new();
    for tombstone in value.try_iter()? {
        let tombstone = tombstone?;
        let operation_id: u64 = item(&tombstone, "operation_id")?.extract()?;
        if !operation_ids.contains(&operation_id) {
            return Err(ProvenanceError::MalformedCapture(format!(
                "tombstone references unknown operation {operation_id}"
            )));
        }
        let kind: String = item(&tombstone, "kind")?.extract()?;
        if !matches!(kind.as_str(), "face" | "edge" | "vertex") {
            return Err(ProvenanceError::MalformedCapture(format!(
                "unknown tombstone topology kind {kind}"
            )));
        }
        tombstones.push(DeletionTombstone {
            operation_id,
            kind,
            ordinal: item(&tombstone, "ordinal")?.extract()?,
        });
    }
    Ok(tombstones)
}

fn item<'py>(value: &Bound<'py, PyAny>, key: &str) -> Result<Bound<'py, PyAny>, ProvenanceError> {
    value.get_item(key).map_err(ProvenanceError::Python)
}

fn parse_operation(value: &str) -> Result<SemanticOperation, ProvenanceError> {
    match value {
        "Box" => Ok(SemanticOperation::Box),
        "Cylinder" => Ok(SemanticOperation::Cylinder),
        "Sphere" => Ok(SemanticOperation::Sphere),
        "Cone" => Ok(SemanticOperation::Cone),
        "Torus" => Ok(SemanticOperation::Torus),
        "Wedge" => Ok(SemanticOperation::Wedge),
        "Extrude" => Ok(SemanticOperation::Extrude),
        "Revolve" => Ok(SemanticOperation::Revolve),
        "Loft" => Ok(SemanticOperation::Loft),
        "Sweep" => Ok(SemanticOperation::Sweep),
        "Thicken" => Ok(SemanticOperation::Thicken),
        "Shell" => Ok(SemanticOperation::Shell),
        "Draft" => Ok(SemanticOperation::Draft),
        "Split" => Ok(SemanticOperation::Split),
        "BooleanFuse" => Ok(SemanticOperation::BooleanFuse),
        "BooleanCut" => Ok(SemanticOperation::BooleanCut),
        "BooleanCommon" => Ok(SemanticOperation::BooleanCommon),
        "Fillet" => Ok(SemanticOperation::Fillet),
        "Chamfer" => Ok(SemanticOperation::Chamfer),
        other => Err(ProvenanceError::MalformedCapture(format!(
            "unknown semantic operation {other}"
        ))),
    }
}

fn parse_relation(value: &str) -> Result<ProvenanceRelation, ProvenanceError> {
    match value {
        "Generated" => Ok(ProvenanceRelation::Generated),
        "Modified" => Ok(ProvenanceRelation::Modified),
        "GeneratedDescendant" => Ok(ProvenanceRelation::GeneratedDescendant),
        "ModifiedDescendant" => Ok(ProvenanceRelation::ModifiedDescendant),
        other => Err(ProvenanceError::MalformedCapture(format!(
            "unknown provenance relation {other}"
        ))),
    }
}

fn build_ledger(raw: &RawProvenance, source: &str) -> Result<ProvenanceLedger, ProvenanceError> {
    let source_lines: Vec<&str> = source.lines().collect();
    let mut operations = HashMap::new();
    for operation in &raw.operations {
        if operation.source_line == 0 || operation.source_line as usize > source_lines.len() {
            return Err(ProvenanceError::MalformedCapture(format!(
                "source line {} is out of range",
                operation.source_line
            )));
        }
        operations.insert(operation.operation_id, operation);
    }

    let mut ledger = ProvenanceLedger::new();

    for (index, element) in raw.faces.iter().enumerate() {
        let value = ledger_value(element, &operations, &source_lines);
        ledger
            .record_face(FaceId(index as u32), value)
            .map_err(|error| ProvenanceError::MalformedCapture(error.to_string()))?;
    }
    for (index, element) in raw.edges.iter().enumerate() {
        let value = ledger_value(element, &operations, &source_lines);
        ledger
            .record_edge(EdgeId(index as u32), value)
            .map_err(|error| ProvenanceError::MalformedCapture(error.to_string()))?;
    }
    for (index, element) in raw.vertices.iter().enumerate() {
        let value = ledger_value(element, &operations, &source_lines);
        ledger
            .record_vertex(VertexId(index as u32), value)
            .map_err(|error| ProvenanceError::MalformedCapture(error.to_string()))?;
    }

    Ok(ledger)
}

/// An element no instrumented operation claimed is recorded as untraced
/// rather than failing the whole execution: the model still renders and the
/// user learns at click time that this one element cannot be sourced.
fn ledger_value(
    element: &RawElement,
    operations: &HashMap<u64, &RawOperation>,
    source_lines: &[&str],
) -> LedgerValue {
    if element.candidates.is_empty() {
        return LedgerValue::Untraced;
    }

    let entries = element
        .candidates
        .iter()
        .map(|candidate| {
            let operation = operations[&candidate.operation_id];
            ProvenanceEntry {
                source: SourceRef {
                    line: operation.source_line,
                    code: source_lines[operation.source_line as usize - 1]
                        .trim()
                        .to_string(),
                },
                operation: operation.operation,
                operation_id: operation.operation_id,
                relation: candidate.relation.clone(),
            }
        })
        .collect::<Vec<_>>();

    if entries.len() == 1 {
        LedgerValue::Resolved(entries.into_iter().next().unwrap())
    } else {
        LedgerValue::Ambiguous(entries)
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use cadmark_core::context::{NullIdentification, resolve_context};
    use cadmark_core::geometry::TopologyElement;
    use pyo3::types::PyDict;

    use super::*;

    fn activate() {
        crate::execution::discover_and_activate_venv().unwrap();
    }

    fn run_instrumentation_source<'py>(
        py: Python<'py>,
    ) -> Result<Bound<'py, PyDict>, ProvenanceError> {
        let namespace = PyDict::new(py);
        let source = CString::new(INSTRUMENTATION_SOURCE).unwrap();
        py.run(&source, Some(&namespace), None)?;
        Ok(namespace)
    }

    fn capture_from_expression<'py>(
        py: Python<'py>,
        expression: &str,
    ) -> Result<Bound<'py, PyAny>, ProvenanceError> {
        let expression = CString::new(expression).unwrap();
        py.eval(&expression, None, None)
            .map_err(ProvenanceError::Python)
    }

    #[test]
    fn manifest_covers_every_supported_binding() {
        activate();
        let _guard = crate::execution::PYTHON_EXECUTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::with_gil(|py| {
            let namespace = run_instrumentation_source(py).unwrap();
            let manifest = namespace.get_item("_cadmark_manifest").unwrap().unwrap();
            assert_eq!(manifest.len().unwrap(), 25);
            let bindings: Vec<(String, String)> = manifest
                .try_iter()
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    (
                        entry.get_item(0).unwrap().extract().unwrap(),
                        entry.get_item(1).unwrap().extract().unwrap(),
                    )
                })
                .collect();
            assert!(bindings.contains(&(
                "build123d.topology.three_d".into(),
                "BRepAlgoAPI_Common".into()
            )));
            assert!(bindings.contains(&(
                "build123d.topology.utils".into(),
                "BRepPrimAPI_MakePrism".into()
            )));
            for expected in [
                "BRepPrimAPI_MakeSphere",
                "BRepPrimAPI_MakeCone",
                "BRepPrimAPI_MakeTorus",
                "BRepPrimAPI_MakeWedge",
                "BRepOffsetAPI_ThruSections",
                "BRepOffsetAPI_MakePipeShell",
                "BRepOffsetAPI_MakeThickSolid",
                "BRepOffset_MakeOffset",
                "BRepOffsetAPI_DraftAngle",
                "BRepAlgoAPI_Splitter",
                "BRepFeat_MakeDPrism",
                "LocOpe_DPrism",
                "BRepBuilderAPI_Transform",
                "BRepBuilderAPI_GTransform",
            ] {
                assert!(
                    bindings.iter().any(|(_, attribute)| attribute == expected),
                    "manifest is missing {expected}"
                );
            }
        });
    }

    #[test]
    fn runtime_drift_is_rejected_before_patching() {
        activate();
        let _guard = crate::execution::PYTHON_EXECUTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::with_gil(|py| {
            let namespace = run_instrumentation_source(py).unwrap();
            let install = namespace.get_item("_cadmark_install").unwrap().unwrap();
            let expected = ((3, 12), "0.11.1", "7.9.3.1", "7.9.3.1.1");
            for actual in [
                ((3, 11), "0.11.1", "7.9.3.1", "7.9.3.1.1"),
                ((3, 12), "0.11.0", "7.9.3.1", "7.9.3.1.1"),
                ((3, 12), "0.11.1", "7.9.2", "7.9.3.1.1"),
                ((3, 12), "0.11.1", "7.9.3.1", "7.9.3.1.0"),
            ] {
                let error = install
                    .call1(("<cadmark-source>", py.None(), actual))
                    .unwrap_err();
                assert_eq!(
                    error.get_type(py).name().unwrap(),
                    "UnsupportedRuntimeVersion"
                );
            }
            assert_eq!(expected.0, (3, 12));
        });
    }

    #[test]
    fn partial_manifest_failure_restores_earlier_binding() {
        activate();
        let _guard = crate::execution::PYTHON_EXECUTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::with_gil(|py| {
            let namespace = run_instrumentation_source(py).unwrap();
            py.run(
                c"
_cadmark_module = _cadmark_importlib.import_module(_cadmark_manifest[0][0])
_cadmark_original = getattr(_cadmark_module, _cadmark_manifest[0][1])
_cadmark_bad = (
    _cadmark_manifest[0],
    ('build123d.topology.three_d', 'MissingCadmarkBinding',
     _cadmark_expected_box, 'Box', 'primitive'),
)
_cadmark_partial_error = ''
try:
    _CadmarkSession('<cadmark-source>').install(_cadmark_bad)
except Exception as exc:
    _cadmark_partial_error = str(exc)
_cadmark_partial_restored = (
    getattr(_cadmark_module, _cadmark_manifest[0][1]) is _cadmark_original
)
",
                Some(&namespace),
                None,
            )
            .unwrap();
            assert!(
                namespace
                    .get_item("_cadmark_partial_restored")
                    .unwrap()
                    .unwrap()
                    .extract::<bool>()
                    .unwrap()
            );
            assert!(
                namespace
                    .get_item("_cadmark_partial_error")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap()
                    .contains("MissingCadmarkBinding")
            );
        });
    }

    #[test]
    fn real_cut_retains_tombstones_outside_the_ledger() {
        activate();
        let _guard = crate::execution::PYTHON_EXECUTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::with_gil(|py| {
            let namespace = PyDict::new(py);
            let session = install_instrumentation(py, &namespace, "<cadmark-source>").unwrap();
            let source = r#"from build123d import *

with BuildPart() as part:
    Box(10, 10, 10)
    with Locations((2, 1, 0)):
        Box(4, 4, 14, mode=Mode.SUBTRACT)
"#;
            let builtins = py.import("builtins").unwrap();
            let code = builtins
                .call_method1("compile", (source, "<cadmark-source>", "exec"))
                .unwrap();
            builtins
                .call_method1("exec", (&code, &namespace, &namespace))
                .unwrap();
            let crate::tessellation::ScriptResult::Solid(shape) =
                crate::tessellation::find_result_shape(&namespace).unwrap()
            else {
                panic!("the probe script builds a solid");
            };
            let (raw, ledger) = finalise(py, &session, &shape, source).unwrap();
            restore(py, &session).unwrap();

            assert!(!raw.tombstones.is_empty());
            assert_eq!(
                ledger.len(),
                raw.faces.len() + raw.edges.len() + raw.vertices.len()
            );
        });
    }

    #[test]
    fn real_cleanup_preserves_distinct_candidates_as_ambiguity() {
        activate();
        let _guard = crate::execution::PYTHON_EXECUTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::with_gil(|py| {
            let namespace = run_instrumentation_source(py).unwrap();
            py.run(
                c"
from OCP.BRepAlgoAPI import BRepAlgoAPI_Fuse as _cadmark_probe_fuse
from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox as _cadmark_probe_box
from OCP.gp import gp_Pnt as _cadmark_probe_point

_cadmark_probe_session = _CadmarkSession('<cadmark-source>')
_cadmark_probe_first = _cadmark_probe_box(1, 1, 1).Shape()
_cadmark_probe_second = _cadmark_probe_box(
    _cadmark_probe_point(1, 0, 0), 1, 1, 1
).Shape()
_cadmark_probe_fuser = _cadmark_probe_fuse(
    _cadmark_probe_first, _cadmark_probe_second
)
_cadmark_probe_unclean = _cadmark_probe_fuser.Shape()
_cadmark_probe_cleanup = _cadmark_expected_cleanup(
    _cadmark_probe_unclean, True, True, True
)
_cadmark_probe_cleanup.Build()
_cadmark_probe_clean = _cadmark_probe_cleanup.Shape()
_cadmark_probe_op1 = _cadmark_probe_session.new_operation(1, 'Box', 'seed-a')
_cadmark_probe_op2 = _cadmark_probe_session.new_operation(2, 'Cylinder', 'seed-b')
_cadmark_probe_history = _cadmark_probe_cleanup.History()
_cadmark_probe_face_inputs = dict(
    _cadmark_probe_session.topology(_cadmark_probe_unclean)
)['face']
_cadmark_probe_face_outputs = dict(
    _cadmark_probe_session.topology(_cadmark_probe_clean)
)['face']
_cadmark_probe_merged_pair = None
for _cadmark_probe_output in _cadmark_probe_face_outputs:
    _cadmark_probe_sources = []
    for _cadmark_probe_input in _cadmark_probe_face_inputs:
        _cadmark_probe_changes = (
            list(_cadmark_probe_history.Modified(_cadmark_probe_input))
            + list(_cadmark_probe_history.Generated(_cadmark_probe_input))
        )
        if any(
            _cadmark_probe_output.IsSame(_cadmark_probe_change)
            for _cadmark_probe_change in _cadmark_probe_changes
        ):
            _cadmark_probe_sources.append(_cadmark_probe_input)
    if len(_cadmark_probe_sources) >= 2:
        _cadmark_probe_merged_pair = _cadmark_probe_sources[:2]
        break
assert _cadmark_probe_merged_pair is not None
for _cadmark_probe_kind, _cadmark_probe_shapes in _cadmark_probe_session.topology(
    _cadmark_probe_unclean
):
    for _cadmark_probe_index, _cadmark_probe_shape in enumerate(_cadmark_probe_shapes):
        _cadmark_probe_session.register(
            _cadmark_probe_shape,
            [{
                'operation_id': (
                    _cadmark_probe_op2
                    if (
                        _cadmark_probe_kind == 'face'
                        and _cadmark_probe_shape.IsSame(
                            _cadmark_probe_merged_pair[1]
                        )
                    )
                    else _cadmark_probe_op1
                ),
                'relation': 'Generated',
            }],
        )
_cadmark_probe_wrapper = type('CleanupProbe', (), {})()
_cadmark_probe_wrapper._cadmark_inputs = [_cadmark_probe_unclean]
_cadmark_probe_wrapper.Modified = _cadmark_probe_cleanup.History().Modified
_cadmark_probe_wrapper.Generated = _cadmark_probe_cleanup.History().Generated
_cadmark_probe_session.capture_transport(
    _cadmark_probe_wrapper, _cadmark_probe_clean, False
)
_cadmark_probe_capture = _cadmark_probe_session.finalise(_cadmark_probe_clean)
",
                Some(&namespace),
                None,
            )
            .unwrap();
            let capture = namespace
                .get_item("_cadmark_probe_capture")
                .unwrap()
                .unwrap();
            let raw = parse_capture(&capture).unwrap();
            let ledger = build_ledger(&raw, "first\nsecond").unwrap();
            let ambiguous_face = ledger
                .face_ids()
                .find(|id| matches!(ledger.lookup_face(*id), Some(LedgerValue::Ambiguous(_))));
            let id = ambiguous_face.expect("real cleanup should merge distinct face candidates");
            let context =
                resolve_context(&TopologyElement::Face(id), &ledger, &NullIdentification).unwrap();
            assert!(context.provenance.candidates().len() >= 2);
            assert!(context.provenance.resolved().is_none());
        });
    }

    #[test]
    fn copy_location_bridge_uses_partner_identity_only_as_a_fallback() {
        activate();
        let _guard = crate::execution::PYTHON_EXECUTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::with_gil(|py| {
            let namespace = run_instrumentation_source(py).unwrap();
            py.run(
                c"
from OCP.BRepBuilderAPI import BRepBuilderAPI_Copy as _cadmark_identity_copy
from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox as _cadmark_identity_box
from OCP.gp import gp_Trsf as _cadmark_identity_trsf, gp_Vec as _cadmark_identity_vec
from OCP.TopLoc import TopLoc_Location as _cadmark_identity_location
from OCP.TopoDS import (
    TopoDS_Builder as _cadmark_identity_builder_type,
    TopoDS_Compound as _cadmark_identity_compound_type,
)

_cadmark_identity_session = _CadmarkSession('<cadmark-source>')
_cadmark_identity_operation = _cadmark_identity_session.new_operation(
    1, 'Box', 'seed'
)
_cadmark_identity_original = _cadmark_identity_box(1, 1, 1).Shape()
for _cadmark_identity_kind, _cadmark_identity_shapes in _cadmark_identity_session.topology(
    _cadmark_identity_original
):
    for _cadmark_identity_shape in _cadmark_identity_shapes:
        _cadmark_identity_session.register(
            _cadmark_identity_shape,
            [{
                'operation_id': _cadmark_identity_operation,
                'relation': 'Generated',
            }],
            allow_partner=True,
        )
_cadmark_identity_copier = _cadmark_identity_copy(_cadmark_identity_original)
_cadmark_identity_copied = _cadmark_identity_copier.Shape()
_cadmark_identity_wrapper = type('CopyProbe', (), {})()
_cadmark_identity_wrapper._cadmark_inputs = [_cadmark_identity_original]
_cadmark_identity_wrapper.Modified = _cadmark_identity_copier.Modified
_cadmark_identity_wrapper.Generated = _cadmark_identity_copier.Generated
_cadmark_identity_session.capture_transport(
    _cadmark_identity_wrapper, _cadmark_identity_copied, True
)
_cadmark_identity_transform = _cadmark_identity_trsf()
_cadmark_identity_transform.SetTranslation(_cadmark_identity_vec(5, 0, 0))
_cadmark_identity_moved = _cadmark_identity_copied.Located(
    _cadmark_identity_location(_cadmark_identity_transform)
)
_cadmark_identity_compound = _cadmark_identity_compound_type()
_cadmark_identity_builder = _cadmark_identity_builder_type()
_cadmark_identity_builder.MakeCompound(_cadmark_identity_compound)
_cadmark_identity_builder.Add(
    _cadmark_identity_compound, _cadmark_identity_copied
)
_cadmark_identity_builder.Add(
    _cadmark_identity_compound, _cadmark_identity_moved
)
_cadmark_identity_capture = _cadmark_identity_session.finalise(
    _cadmark_identity_compound
)
_cadmark_identity_is_same = _cadmark_identity_copied.IsSame(
    _cadmark_identity_moved
)
_cadmark_identity_is_partner = _cadmark_identity_copied.IsPartner(
    _cadmark_identity_moved
)
",
                Some(&namespace),
                None,
            )
            .unwrap();
            assert!(
                !namespace
                    .get_item("_cadmark_identity_is_same")
                    .unwrap()
                    .unwrap()
                    .extract::<bool>()
                    .unwrap()
            );
            assert!(
                namespace
                    .get_item("_cadmark_identity_is_partner")
                    .unwrap()
                    .unwrap()
                    .extract::<bool>()
                    .unwrap()
            );
            let capture = namespace
                .get_item("_cadmark_identity_capture")
                .unwrap()
                .unwrap();
            let raw = parse_capture(&capture).unwrap();
            assert_eq!(raw.faces.len(), 12);
            assert_eq!(raw.edges.len(), 24);
            assert_eq!(raw.vertices.len(), 16);
            assert!(
                raw.faces
                    .iter()
                    .chain(&raw.edges)
                    .chain(&raw.vertices)
                    .all(|element| element.candidates.len() == 1)
            );
        });
    }

    #[test]
    fn malformed_capture_variants_fail_closed() {
        Python::with_gil(|py| {
            let baseline = "{'schema_version': 1, 'operations': [{'operation_id': 1, \
                'source_line': 1, 'operation': 'Box', 'api_class': 'Box'}], \
                'faces': [{'candidates': [{'operation_id': 1, 'relation': 'Generated'}]}], \
                'edges': [{'candidates': [{'operation_id': 1, 'relation': 'Generated'}]}], \
                'vertices': [{'candidates': [{'operation_id': 1, 'relation': 'Generated'}]}], \
                'tombstones': []}";
            let valid = capture_from_expression(py, baseline).unwrap();
            let raw = parse_capture(&valid).unwrap();
            assert!(build_ledger(&raw, "Box()").is_ok());

            let unknown_schema = capture_from_expression(
                py,
                &baseline.replace("'schema_version': 1", "'schema_version': 2"),
            )
            .unwrap();
            assert!(matches!(
                parse_capture(&unknown_schema),
                Err(ProvenanceError::MalformedCapture(_))
            ));

            let duplicate_operations = capture_from_expression(
                py,
                &baseline.replace(
                    "'operations': [",
                    "'operations': [{'operation_id': 1, 'source_line': 1, \
                     'operation': 'Box', 'api_class': 'Duplicate'}, ",
                ),
            )
            .unwrap();
            assert!(matches!(
                parse_capture(&duplicate_operations),
                Err(ProvenanceError::MalformedCapture(_))
            ));

            let unknown_operation = capture_from_expression(
                py,
                &baseline.replacen("'operation_id': 1", "'operation_id': 99", 2),
            )
            .unwrap();
            assert!(matches!(
                parse_capture(&unknown_operation),
                Err(ProvenanceError::MalformedCapture(_))
            ));

            let duplicate_candidate = capture_from_expression(
                py,
                &baseline.replace(
                    "'candidates': [{'operation_id': 1, 'relation': 'Generated'}]",
                    "'candidates': [{'operation_id': 1, 'relation': 'Generated'}, \
                     {'operation_id': 1, 'relation': 'Generated'}]",
                ),
            )
            .unwrap();
            assert!(matches!(
                parse_capture(&duplicate_candidate),
                Err(ProvenanceError::MalformedCapture(_))
            ));

            let empty_candidates = capture_from_expression(
                py,
                &baseline.replacen(
                    "'candidates': [{'operation_id': 1, 'relation': 'Generated'}]",
                    "'candidates': []",
                    1,
                ),
            )
            .unwrap();
            let raw = parse_capture(&empty_candidates).unwrap();
            let ledger = build_ledger(&raw, "Box()").unwrap();
            assert_eq!(ledger.lookup_face(FaceId(0)), Some(&LedgerValue::Untraced));
            assert_eq!(ledger.untraced_count(), 1);

            let out_of_range = capture_from_expression(
                py,
                &baseline.replace("'source_line': 1", "'source_line': 2"),
            )
            .unwrap();
            let raw = parse_capture(&out_of_range).unwrap();
            assert!(matches!(
                build_ledger(&raw, "Box()"),
                Err(ProvenanceError::MalformedCapture(_))
            ));
        });
    }

    #[test]
    fn restoration_failure_poisons_later_executions() {
        const CHILD_ENV: &str = "CADMARK_PROVENANCE_POISON_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "provenance::tests::restoration_failure_poisons_later_executions",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }

        activate();
        let _guard = crate::execution::PYTHON_EXECUTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::with_gil(|py| {
            let namespace = PyDict::new(py);
            let session = install_instrumentation(py, &namespace, "<cadmark-source>").unwrap();
            py.run(
                c"
class _CadmarkBadRestore:
    def __setattr__(self, _name, _value):
        raise RuntimeError('forced restoration failure')
_cadmark_bad_restore = _CadmarkBadRestore()
",
                Some(&namespace),
                None,
            )
            .unwrap();
            let bad = namespace.get_item("_cadmark_bad_restore").unwrap().unwrap();
            session
                .inner
                .bind(py)
                .getattr("originals")
                .unwrap()
                .call_method1("append", ((bad, "binding", py.None()),))
                .unwrap();
            assert!(matches!(
                restore(py, &session),
                Err(ProvenanceError::WrapperRestoration(_))
            ));
        });
        drop(_guard);
        let scratch = tempfile::tempdir().unwrap();
        assert!(matches!(
            crate::execution::execute_script_source("value = 1", scratch.path()),
            Err(crate::execution::ExecutionError::Provenance(
                ProvenanceError::Poisoned
            ))
        ));
    }
}
