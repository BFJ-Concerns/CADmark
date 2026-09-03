// Export of the executed model to interchange and slicer formats.

use std::path::Path;
use std::sync::Arc;

use cadmark_core::export::ExportFormat;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExportError {
    #[error("export failed: {0}")]
    Python(#[from] PyErr),
    #[error("the exporter reported failure writing {0}")]
    WriteFailed(String),
}

/// The build123d object produced by the last successful execution, retained so
/// it can be exported without re-running the script.
///
/// Cloning shares the handle without touching the interpreter, so a clone can
/// be taken on the UI thread while the worker holds the GIL.
#[derive(Clone)]
pub struct ModelHandle(Arc<Py<PyAny>>);

impl ModelHandle {
    pub(crate) fn new(object: Py<PyAny>) -> Self {
        Self(Arc::new(object))
    }
}

impl std::fmt::Debug for ModelHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ModelHandle")
    }
}

const EXPORT_SOURCE: &std::ffi::CStr = c"
from build123d import Mesher, export_step, export_stl


def export_model(shape, format_name, path):
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
    model: &ModelHandle,
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
                model.0.bind(py),
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
