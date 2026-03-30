// OCP instrumentation for provenance capture.
//
// Wraps OCP builder classes during script execution to record which
// source lines generated which topological elements. Uses OCCT's
// BRepBuilderAPI Modified/Generated/IsDeleted history interface.

use pyo3::prelude::*;
use pyo3::types::PyDict;
use pyo3::IntoPyObjectExt;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProvenanceError {
    #[error("Python error during provenance capture: {0}")]
    Python(#[from] PyErr),
    #[error("Failed to extract provenance data: {0}")]
    Extraction(String),
}

/// Raw provenance data captured during a single script execution.
/// Maps OCP shape hashes to the source lines that produced them.
#[derive(Debug, Default)]
pub struct RawProvenance {
    /// (shape_hash, source_line, kind) tuples captured during execution.
    pub entries: Vec<RawProvenanceEntry>,
}

#[derive(Debug)]
pub struct RawProvenanceEntry {
    /// Hash of the OCP TopoDS_Shape.
    pub shape_hash: u64,
    /// 1-indexed line number in the executed script.
    pub source_line: u32,
    /// Whether this shape was Generated or Modified by the operation.
    pub kind: ProvenanceRelation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvenanceRelation {
    Generated,
    Modified,
}

/// Python source for the OCP instrumentation wrapper.
/// Injected into the script's execution namespace before running.
const INSTRUMENTATION_SOURCE: &std::ffi::CStr = c"
import inspect as _cadmark_inspect

# Accumulator for provenance records — populated by wrapped builders.
_cadmark_provenance = []

def _cadmark_wrap_builder(original_cls, cls_name):
    \"\"\"Wrap an OCP builder class to capture provenance on Build().\"\"\"
    class WrappedBuilder(original_cls):
        def __init__(self, *args, **kwargs):
            # Capture the call site — skip this wrapper frame.
            frame = _cadmark_inspect.stack()[1]
            self._cadmark_source_line = frame.lineno
            super().__init__(*args, **kwargs)

        def Build(self, *args, **kwargs):
            result = super().Build(*args, **kwargs)
            shape = self.Shape()
            _cadmark_provenance.append({
                'shape_hash': hash(shape.IsNull()) if shape.IsNull() else shape.HashCode(2**31 - 1),
                'source_line': self._cadmark_source_line,
                'kind': 'generated',
                'builder': cls_name,
            })
            return result

    WrappedBuilder.__name__ = cls_name
    return WrappedBuilder
";

/// Inject provenance instrumentation into the Python execution environment.
/// Returns a handle used to extract results after execution.
pub fn inject_instrumentation(py: Python<'_>) -> Result<Py<PyAny>, ProvenanceError> {
    // Run the instrumentation setup code.
    let globals = PyDict::new(py);
    py.run(INSTRUMENTATION_SOURCE, Some(&globals), None)?;

    // Return the provenance accumulator so we can read it after execution.
    let accumulator = globals
        .get_item("_cadmark_provenance")?
        .ok_or_else(|| ProvenanceError::Extraction("provenance accumulator not found".into()))?;

    Ok(accumulator.into_py_any(py)?)
}

/// Extract provenance data after script execution completes.
pub fn extract_provenance(
    py: Python<'_>,
    accumulator: &Py<PyAny>,
) -> Result<RawProvenance, ProvenanceError> {
    let list = accumulator.bind(py);
    let mut entries = Vec::new();

    for item in list.try_iter()? {
        let item = item?;
        let shape_hash: u64 = item.get_item("shape_hash")?.extract()?;
        let source_line: u32 = item.get_item("source_line")?.extract()?;
        let kind_str: String = item.get_item("kind")?.extract()?;
        let kind = match kind_str.as_str() {
            "modified" => ProvenanceRelation::Modified,
            _ => ProvenanceRelation::Generated,
        };

        entries.push(RawProvenanceEntry {
            shape_hash,
            source_line,
            kind,
        });
    }

    Ok(RawProvenance { entries })
}
