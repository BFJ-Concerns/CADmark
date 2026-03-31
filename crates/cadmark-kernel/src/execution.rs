// Script execution — runs build123d Python scripts and captures results.

use std::ffi::CString;
use std::path::Path;
use std::sync::Once;

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

/// Activate a Python virtualenv for the embedded interpreter.
///
/// Adds the venv's site-packages to `sys.path` so that packages
/// installed in the venv (e.g. build123d) are importable. Call
/// once at startup — repeated calls are no-ops.
pub fn activate_venv(venv_path: &Path) -> Result<(), ExecutionError> {
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
        log::warn!(
            "No site-packages found in venv at {}",
            venv_path.display()
        );
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

        // Inject provenance instrumentation into the execution namespace.
        let provenance_capture = crate::provenance::inject_instrumentation(py)?;

        // Execute the script in an isolated namespace.
        let globals = PyDict::new(py);
        let c_source = CString::new(source)
            .map_err(|e| ExecutionError::ScriptNotFound(format!("invalid source: {e}")))?;

        log::info!("Running script via py.run() ({} bytes)...", source.len());
        py.run(&c_source, Some(&globals), None).map_err(|e| {
            log::error!("Python execution failed: {e}");
            ExecutionError::Python(e)
        })?;

        // Log what's in the namespace — crucial for diagnosing shape
        // detection failures. At info level because this is the main
        // debugging tool until the app is stable.
        let keys: Vec<String> = globals
            .keys()
            .into_iter()
            .filter_map(|k| k.extract::<String>().ok())
            .filter(|k| !k.starts_with("__"))
            .collect();
        log::info!(
            "Script executed. Namespace has {} user entries: [{}]",
            keys.len(),
            keys.join(", "),
        );

        // Extract provenance data from the instrumentation hooks.
        let provenance = crate::provenance::extract_provenance(py, &provenance_capture)?;

        // Find the result solid and tessellate it.
        let mesh = crate::tessellation::tessellate_from_namespace(py, &globals)?;

        Ok(ExecutionResult { mesh, provenance })
    })
}
