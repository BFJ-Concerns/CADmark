// Export of a kept model to interchange and slicer formats.
//
// Runs inside the kernel worker: the model is the BREP file an execution
// left in the worker's scratch directory, re-read here so an export never
// depends on the script running again.

use std::path::Path;

use cadmark_core::export::ExportFormat;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use thiserror::Error;

use crate::protocol::ModelFile;

#[derive(Error, Debug)]
pub enum ExportError {
    #[error("export failed: {0}")]
    Python(#[from] PyErr),
    #[error("the exporter reported failure writing {0}")]
    WriteFailed(String),
}

const EXPORT_SOURCE: &std::ffi::CStr = c"
from build123d import Mesher, export_step, export_stl, import_brep


def export_model(model_path, format_name, path):
    shape = import_brep(model_path)
    if format_name == 'step':
        return export_step(shape, path)
    if format_name == 'stl':
        return export_stl(shape, path)
    if format_name == '3mf':
        mesher = Mesher()
        mesher.add_shape(shape)
        mesher.write(path)
        return True
    raise ValueError(f'unknown export format {format_name}')
";

/// Write the model to `path` in the given format.
///
/// Takes the kernel execution lock: build123d's exporters construct OCP
/// builders, which must not run while another execution has them wrapped.
pub fn export_model(
    model: &ModelFile,
    format: ExportFormat,
    path: &Path,
) -> Result<(), ExportError> {
    let _execution_guard = crate::execution::PYTHON_EXECUTION_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Python::with_gil(|py| {
        let namespace = PyDict::new(py);
        py.run(EXPORT_SOURCE, Some(&namespace), None)?;
        let export = namespace
            .get_item("export_model")?
            .expect("export source defines export_model()");
        let succeeded: bool = export
            .call1((
                model.0.display().to_string(),
                format.extension(),
                path.display().to_string(),
            ))?
            .extract()?;
        if succeeded {
            Ok(())
        } else {
            Err(ExportError::WriteFailed(path.display().to_string()))
        }
    })
}
