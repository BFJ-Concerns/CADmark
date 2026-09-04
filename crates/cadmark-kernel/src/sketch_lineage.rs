//! The sketch half of one capture, read into the consumer's ledger.
//!
//! The instrumentation labels every stock sketch object with the line that
//! drew it and moves those labels onto final topology only where a maker
//! reported the relation. This module turns that into the ledger the
//! application and the model read: a drawn curve where one was reached, and
//! otherwise the stated reason none was — the operation that answered
//! nothing, or the clean-up step that keeps no history at all.

use std::collections::HashMap;

use cadmark_core::geometry::{EdgeId, FaceId, SketchElement, SketchElementKind, VertexId};
use cadmark_core::ledger::SourceRef;
use cadmark_core::sketch_lineage::{
    NoSketchRoute, SketchLineage, SketchLineageLedger, SketchSource,
};
use pyo3::prelude::*;

use crate::provenance::{ProvenanceError, RawOperation};

/// Build the sketch lineage ledger from a finalised capture.
pub(crate) fn build_ledger(
    capture: &Bound<'_, PyAny>,
    operations: &[RawOperation],
    source: &str,
) -> Result<SketchLineageLedger, ProvenanceError> {
    let source_lines: Vec<&str> = source.lines().collect();
    let operations: HashMap<u64, &RawOperation> = operations
        .iter()
        .map(|operation| (operation.operation_id, operation))
        .collect();

    let sketches = parse_sketches(&item(capture, "sketches")?, &source_lines)?;

    let mut ledger = SketchLineageLedger::new();
    for (element, sketch_id) in parse_sketch_elements(&item(capture, "sketch_elements")?)? {
        ledger.record_element(element, sketch_source(sketch_id, &sketches)?);
    }

    for (index, element) in item(capture, "faces")?.try_iter()?.enumerate() {
        let lineage = parse_lineage(&element?, &sketches, &operations, &source_lines)?;
        ledger.record_face(FaceId(index as u32), lineage);
    }
    for (index, element) in item(capture, "edges")?.try_iter()?.enumerate() {
        let lineage = parse_lineage(&element?, &sketches, &operations, &source_lines)?;
        ledger.record_edge(EdgeId(index as u32), lineage);
    }
    for (index, element) in item(capture, "vertices")?.try_iter()?.enumerate() {
        let lineage = parse_lineage(&element?, &sketches, &operations, &source_lines)?;
        ledger.record_vertex(VertexId(index as u32), lineage);
    }

    Ok(ledger)
}

fn parse_sketches(
    value: &Bound<'_, PyAny>,
    source_lines: &[&str],
) -> Result<HashMap<u64, SketchSource>, ProvenanceError> {
    let mut sketches = HashMap::new();
    for sketch in value.try_iter()? {
        let sketch = sketch?;
        let sketch_id: u64 = item(&sketch, "sketch_id")?.extract()?;
        let line: u32 = item(&sketch, "source_line")?.extract()?;
        if line == 0 || line as usize > source_lines.len() {
            return Err(ProvenanceError::MalformedCapture(format!(
                "sketch source line {line} is out of range"
            )));
        }
        let source = SketchSource {
            source: SourceRef {
                line,
                code: source_lines[line as usize - 1].trim().to_string(),
            },
            object: item(&sketch, "object")?.extract()?,
        };
        if sketches.insert(sketch_id, source).is_some() {
            return Err(ProvenanceError::MalformedCapture(format!(
                "duplicate sketch object {sketch_id}"
            )));
        }
    }
    Ok(sketches)
}

/// Sketch elements are numbered per kind in the order the script drew
/// them, so a curve's index does not shift when a corner is added.
fn parse_sketch_elements(
    value: &Bound<'_, PyAny>,
) -> Result<Vec<(SketchElement, u64)>, ProvenanceError> {
    let mut next_index: HashMap<SketchElementKind, u32> = HashMap::new();
    let mut elements = Vec::new();
    for entry in value.try_iter()? {
        let entry = entry?;
        let kind = parse_element_kind(&item(&entry, "kind")?.extract::<String>()?)?;
        let index = next_index.entry(kind).or_insert(0);
        elements.push((SketchElement { kind, index: *index }, item(&entry, "sketch_id")?.extract()?));
        *index += 1;
    }
    Ok(elements)
}

fn parse_element_kind(value: &str) -> Result<SketchElementKind, ProvenanceError> {
    match value {
        "curve" => Ok(SketchElementKind::Curve),
        "corner" => Ok(SketchElementKind::Corner),
        "region" => Ok(SketchElementKind::Region),
        other => Err(ProvenanceError::MalformedCapture(format!(
            "unknown sketch element kind {other}"
        ))),
    }
}

fn sketch_source(
    sketch_id: u64,
    sketches: &HashMap<u64, SketchSource>,
) -> Result<SketchSource, ProvenanceError> {
    sketches.get(&sketch_id).cloned().ok_or_else(|| {
        ProvenanceError::MalformedCapture(format!("unknown sketch object {sketch_id}"))
    })
}

/// One final element's sketch route. An element with neither a label nor a
/// barrier had no sketch anywhere in its construction; that is a stated
/// absence too, not a missing answer.
fn parse_lineage(
    element: &Bound<'_, PyAny>,
    sketches: &HashMap<u64, SketchSource>,
    operations: &HashMap<u64, &RawOperation>,
    source_lines: &[&str],
) -> Result<SketchLineage, ProvenanceError> {
    let state = item(element, "sketch")?;

    if let Some(candidates) = optional(&state, "candidates")? {
        let mut sources = Vec::new();
        for candidate in candidates.try_iter()? {
            sources.push(sketch_source(candidate?.extract()?, sketches)?);
        }
        return Ok(match sources.len() {
            0 => {
                return Err(ProvenanceError::MalformedCapture(
                    "sketch ancestry with no candidate".to_string(),
                ));
            }
            1 => SketchLineage::Resolved(sources.into_iter().next().unwrap()),
            _ => SketchLineage::Ambiguous(sources),
        });
    }

    if let Some(barrier) = optional(&state, "barrier")? {
        return Ok(SketchLineage::NoRoute(parse_barrier(
            &barrier,
            operations,
            source_lines,
        )?));
    }

    Ok(SketchLineage::NoRoute(NoSketchRoute::NoSketchAncestor))
}

fn parse_barrier(
    barrier: &Bound<'_, PyAny>,
    operations: &HashMap<u64, &RawOperation>,
    source_lines: &[&str],
) -> Result<NoSketchRoute, ProvenanceError> {
    match item(barrier, "reason")?.extract::<String>()?.as_str() {
        "clean_up" => Ok(NoSketchRoute::CleanUpStep),
        "history_empty" => {
            let operation_id: u64 = item(barrier, "operation_id")?.extract()?;
            let operation = operations.get(&operation_id).ok_or_else(|| {
                ProvenanceError::MalformedCapture(format!(
                    "sketch barrier references unknown operation {operation_id}"
                ))
            })?;
            let line = operation.source_line;
            if line == 0 || line as usize > source_lines.len() {
                return Err(ProvenanceError::MalformedCapture(format!(
                    "sketch barrier source line {line} is out of range"
                )));
            }
            Ok(NoSketchRoute::OperationHistoryEmpty {
                operation: operation.operation,
                source: SourceRef {
                    line,
                    code: source_lines[line as usize - 1].trim().to_string(),
                },
            })
        }
        other => Err(ProvenanceError::MalformedCapture(format!(
            "unknown sketch barrier reason {other}"
        ))),
    }
}

fn item<'py>(value: &Bound<'py, PyAny>, key: &str) -> Result<Bound<'py, PyAny>, ProvenanceError> {
    value.get_item(key).map_err(ProvenanceError::Python)
}

/// A capture key that is present only in some states.
fn optional<'py>(
    value: &Bound<'py, PyAny>,
    key: &str,
) -> Result<Option<Bound<'py, PyAny>>, ProvenanceError> {
    match value.get_item(key) {
        Ok(found) => Ok(Some(found)),
        Err(error) if error.is_instance_of::<pyo3::exceptions::PyKeyError>(value.py()) => Ok(None),
        Err(error) => Err(ProvenanceError::Python(error)),
    }
}
