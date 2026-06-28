#[allow(dead_code)]
#[path = "src/python_runtime.rs"]
mod python_runtime;

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
    println!("cargo:rerun-if-changed=src/python_runtime.rs");

    let Some(interpreter_path) = std::env::var_os("PYO3_PYTHON") else {
        println!(
            "cargo:warning=PYO3_PYTHON is not set; embedded Python home will not be baked into cadmark-kernel"
        );
        return;
    };

    let interpreter_path = Path::new(&interpreter_path);
    let layout = python_runtime::derive_runtime_layout_from_interpreter(interpreter_path)
        .unwrap_or_else(|error| {
            panic!(
                "Failed to derive embedded Python runtime from PYO3_PYTHON ({}): {error}",
                interpreter_path.display()
            )
        });

    if !layout.lib_dir.is_dir() {
        panic!(
            "Configured embedded Python lib directory does not exist: {}",
            layout.lib_dir.display()
        );
    }

    println!(
        "cargo:rustc-env=CADMARK_EMBEDDED_PYTHON_HOME={}",
        layout.home.display()
    );
    println!(
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        layout.lib_dir.display()
    );
}
