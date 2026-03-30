// Script execution — runs build123d Python scripts and captures results.

use std::ffi::CString;
use std::path::Path;

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

/// Result of executing a build123d script.
pub struct ExecutionResult {
    /// Tessellated mesh for the renderer.
    pub mesh: TessellatedMesh,
    /// Provenance data captured during execution.
    pub provenance: crate::provenance::RawProvenance,
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

    execute_script_source(&script_content)
}

/// Execute build123d source code directly (for testing and AI-generated code).
pub fn execute_script_source(source: &str) -> Result<ExecutionResult, ExecutionError> {
    Python::with_gil(|py| {
        // Inject provenance instrumentation into the execution namespace.
        let provenance_capture = crate::provenance::inject_instrumentation(py)?;

        // Execute the script in an isolated namespace.
        let globals = PyDict::new(py);
        let c_source = CString::new(source)
            .map_err(|e| ExecutionError::ScriptNotFound(format!("invalid source: {e}")))?;
        py.run(&c_source, Some(&globals), None)?;

        // Extract provenance data from the instrumentation hooks.
        let provenance = crate::provenance::extract_provenance(py, &provenance_capture)?;

        // Find the result solid and tessellate it.
        let mesh = crate::tessellation::tessellate_from_namespace(py, &globals)?;

        Ok(ExecutionResult { mesh, provenance })
    })
}
