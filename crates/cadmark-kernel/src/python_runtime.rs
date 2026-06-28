// Embedded Python runtime discovery shared between build scripts and runtime.
//
// CADmark embeds CPython via PyO3 and currently supports a uv-managed
// Python 3.12 runtime. The final executable must be able to recover the
// Python home from the configured interpreter path so it can set
// `PYTHONHOME` before the interpreter initialises.

use std::path::{Path, PathBuf};
use std::sync::Once;

/// Environment variable baked in by the kernel build script.
const EMBEDDED_PYTHON_HOME: Option<&str> = option_env!("CADMARK_EMBEDDED_PYTHON_HOME");

/// One-time guard for configuring `PYTHONHOME`.
static PYTHON_HOME_CONFIGURED: Once = Once::new();

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonRuntimeLayout {
    pub home: PathBuf,
    pub lib_dir: PathBuf,
}

/// Derive the embedded Python home and lib directory from the interpreter path.
#[cfg_attr(not(test), allow(dead_code))]
pub fn derive_runtime_layout_from_interpreter(
    interpreter_path: &Path,
) -> Result<PythonRuntimeLayout, String> {
    let Some(bin_dir) = interpreter_path.parent() else {
        return Err(format!(
            "Python interpreter path has no parent directory: {}",
            interpreter_path.display()
        ));
    };

    let Some(home) = bin_dir.parent() else {
        return Err(format!(
            "Python interpreter path does not live under a Python home: {}",
            interpreter_path.display()
        ));
    };

    if bin_dir.file_name().and_then(|name| name.to_str()) != Some("bin") {
        return Err(format!(
            "Python interpreter path must live in a bin directory: {}",
            interpreter_path.display()
        ));
    }

    Ok(PythonRuntimeLayout {
        home: home.to_path_buf(),
        lib_dir: home.join("lib"),
    })
}

/// Set `PYTHONHOME` from the compile-time embedded runtime if the caller has
/// not already supplied one.
pub fn configure_python_home() {
    PYTHON_HOME_CONFIGURED.call_once(|| {
        if std::env::var_os("PYTHONHOME").is_some() {
            return;
        }

        let Some(python_home) = EMBEDDED_PYTHON_HOME else {
            return;
        };

        // SAFETY: This runs during process start-up before CADmark starts any
        // threads or initialises Python, so there is no concurrent environment
        // access to race with.
        unsafe {
            std::env::set_var("PYTHONHOME", python_home);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::derive_runtime_layout_from_interpreter;
    use std::path::Path;

    #[test]
    fn derives_runtime_layout_from_uv_python_interpreter() {
        let layout = derive_runtime_layout_from_interpreter(Path::new(
            "/home/user/.local/share/uv/python/cpython-3.12-linux-x86_64-gnu/bin/python3.12",
        ))
        .expect("expected uv-managed interpreter layout");

        assert_eq!(
            layout.home,
            Path::new("/home/user/.local/share/uv/python/cpython-3.12-linux-x86_64-gnu")
        );
        assert_eq!(
            layout.lib_dir,
            Path::new("/home/user/.local/share/uv/python/cpython-3.12-linux-x86_64-gnu/lib")
        );
    }

    #[test]
    fn rejects_interpreter_path_outside_bin_directory() {
        let error = derive_runtime_layout_from_interpreter(Path::new(
            "/home/user/.local/share/uv/python/cpython-3.12-linux-x86_64-gnu/python3.12",
        ))
        .expect_err("expected invalid interpreter layout");

        assert!(error.contains("bin directory"));
    }
}
