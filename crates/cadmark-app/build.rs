// Embed an rpath to the Python runtime's shared library in the application
// binary so it loads libpython without an environment. The kernel crate's
// build script does the same for the worker binary. The interpreter is
// found the same way there.

#[allow(dead_code)]
#[path = "../cadmark-kernel/src/python_runtime.rs"]
mod python_runtime;

fn main() {
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
    println!("cargo:rerun-if-changed=../cadmark-kernel/src/python_runtime.rs");

    let Some(interpreter_path) = python_runtime::configured_interpreter() else {
        println!(
            "cargo:warning=PYO3_PYTHON is not set or does not exist; run scripts/bootstrap-python-runtime. cadmark will not embed an rpath for libpython"
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
        "cargo:rustc-link-arg-bin=cadmark=-Wl,-rpath,{}",
        layout.lib_dir.display()
    );
}
