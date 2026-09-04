// Bake the embedded Python runtime's home into the kernel so PYTHONHOME
// can be set before the interpreter initialises, and link an rpath to its
// shared library. The interpreter is PYO3_PYTHON (supplied by
// `.cargo/config.toml` as the project's `.venv/bin/python`, or by the
// environment); its symlink is followed to the real install.

#[allow(dead_code)]
#[path = "src/python_runtime.rs"]
mod python_runtime;

fn main() {
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
    println!("cargo:rerun-if-changed=src/python_runtime.rs");

    let Some(interpreter_path) = python_runtime::configured_interpreter() else {
        println!(
            "cargo:warning=PYO3_PYTHON is not set or does not exist; run scripts/bootstrap-python-runtime. The embedded Python home will not be baked into cadmark-kernel"
        );
        return;
    };

    let layout = python_runtime::derive_runtime_layout_from_interpreter(&interpreter_path)
        .unwrap_or_else(|error| {
            panic!(
                "Failed to derive embedded Python runtime from {}: {error}",
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
