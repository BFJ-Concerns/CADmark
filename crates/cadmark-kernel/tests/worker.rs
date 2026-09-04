// The kernel worker, driven as the application drives it: the real
// `cadmark-kernel-worker` binary, a real project folder, the real Python
// runtime. These are the "kernel, called as the app calls it" rung's
// proofs of the execution boundary: limits trip, confinement holds, a
// cancel stops the script, and a killed worker is replaced.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cadmark_core::cancellation::CancelFlag;
use cadmark_core::export::ExportFormat;
use cadmark_core::limits::{ExecutionLimits, LimitHit};
use cadmark_kernel::worker::{KernelWorker, WorkerError, WorkerLaunch};

/// The worker binary Cargo built for this test run.
fn worker_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cadmark-kernel-worker"))
}

/// The project's virtual environment, the same one the in-process tests
/// activate.
fn venv() -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    workspace.join(".venv")
}

/// A project folder holding one script, and a worker pointed at it.
fn project_with_script(source: &str) -> (tempfile::TempDir, PathBuf, KernelWorker) {
    let project = tempfile::tempdir().unwrap();
    let script = project.path().join("part.py");
    std::fs::write(&script, source).unwrap();
    let worker = KernelWorker::new(WorkerLaunch {
        binary: worker_binary(),
        project_dir: project.path().to_path_buf(),
        venv: Some(venv()),
    });
    (project, script, worker)
}

/// Generous ceilings for scripts expected to finish.
fn roomy() -> ExecutionLimits {
    ExecutionLimits {
        wall_clock: Duration::from_secs(120),
        memory_bytes: 8 * 1024 * 1024 * 1024,
    }
}

const BOX: &str = "from build123d import *\n\nwith BuildPart() as part:\n    Box(10, 10, 10)\n";

#[test]
fn executes_a_script_and_exports_its_kept_model() {
    let (project, script, mut worker) = project_with_script(BOX);
    let model = worker.execute(&script, roomy(), &CancelFlag::new()).unwrap();
    assert_eq!(model.ledger.face_count(), 6);
    assert_eq!(model.summary.face_count, 6);
    assert!(model.is_printable());
    assert!(model.model.0.is_file(), "model kept at {:?}", model.model);

    let export = project.path().join("part.stl");
    worker
        .export(&model.model, ExportFormat::Stl, &export, roomy())
        .unwrap();
    assert!(std::fs::metadata(&export).unwrap().len() > 0);
}

#[test]
fn a_script_fault_is_reported_with_the_users_own_line() {
    let (_project, script, mut worker) = project_with_script(
        "from build123d import *\n\nwith BuildPart() as part:\n    Box(1, 1, 1)\nraise RuntimeError('deliberate')\n",
    );
    let error = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap_err();
    assert!(error.is_script_fault(), "{error}");
    let message = error.to_string();
    assert!(message.contains("line 5"), "{message}");
    assert!(message.contains("deliberate"), "{message}");

    // A script fault leaves the worker serving: the next execution runs on
    // the same process.
    let model = worker
        .execute(&script_path_with(&_project, BOX), roomy(), &CancelFlag::new())
        .unwrap();
    assert_eq!(model.ledger.face_count(), 6);
}

#[test]
fn a_syntax_error_is_a_script_fault_the_ai_can_fix() {
    let (_project, script, mut worker) =
        project_with_script("from build123d import *\n\nwith BuildPart() as part\n    Box(1, 1, 1)\n");
    let error = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap_err();
    assert!(error.is_script_fault(), "{error}");
    assert!(error.to_string().contains("SyntaxError"), "{error}");
}

#[test]
fn a_runaway_script_is_stopped_at_the_wall_clock_limit() {
    let (_project, script, mut worker) =
        project_with_script("import time\nwhile True:\n    time.sleep(0.05)\n");
    let limits = ExecutionLimits {
        wall_clock: Duration::from_secs(2),
        ..roomy()
    };
    let started = std::time::Instant::now();
    let error = worker
        .execute(&script, limits, &CancelFlag::new())
        .unwrap_err();
    assert!(
        matches!(
            error,
            WorkerError::Limit {
                hit: LimitHit::WallClock,
                ..
            }
        ),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(error.is_script_fault());
    assert!(error.to_string().contains("2 s"), "{error}");

    // The killed worker is replaced on the next request.
    let model = worker
        .execute(&script_path_with(&_project, BOX), roomy(), &CancelFlag::new())
        .unwrap();
    assert_eq!(model.ledger.face_count(), 6);
}

#[test]
fn a_memory_hungry_script_is_stopped_at_the_memory_limit() {
    let (_project, script, mut worker) = project_with_script(
        "hoard = []\nwhile True:\n    hoard.append(bytearray(64 * 1024 * 1024))\n",
    );
    // The runtime itself is resident at well under this after import; a
    // script that keeps allocating crosses it within a few iterations.
    let limits = ExecutionLimits {
        wall_clock: Duration::from_secs(60),
        memory_bytes: 1024 * 1024 * 1024,
    };
    let error = worker
        .execute(&script, limits, &CancelFlag::new())
        .unwrap_err();
    assert!(
        matches!(
            error,
            WorkerError::Limit {
                hit: LimitHit::Memory,
                ..
            }
        ),
        "{error}"
    );
    assert!(error.to_string().contains("1024 MB"), "{error}");
}

#[test]
fn executed_code_cannot_read_outside_the_project_folder() {
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "SECRET_OUTSIDE").unwrap();
    let (_project, script, mut worker) = project_with_script(&format!(
        "open({:?}).read()\n",
        outside.path().display().to_string()
    ));
    let error = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap_err();
    assert!(error.is_script_fault(), "{error}");
    assert!(error.to_string().contains("PermissionError"), "{error}");
}

#[test]
fn executed_code_can_read_and_write_inside_the_project_folder() {
    let (project, script, mut worker) = project_with_script(
        "from build123d import *\nopen('note.txt', 'w').write('hello')\nassert open('note.txt').read() == 'hello'\n\nwith BuildPart() as part:\n    Box(2, 2, 2)\n",
    );
    // The worker's working directory is the project folder, so a relative
    // path in the script lands there.
    let model = worker.execute(&script, roomy(), &CancelFlag::new()).unwrap();
    assert_eq!(model.ledger.face_count(), 6);
    assert_eq!(
        std::fs::read_to_string(project.path().join("note.txt")).unwrap(),
        "hello"
    );
}

#[test]
fn executed_code_cannot_open_a_socket() {
    let (_project, script, mut worker) =
        project_with_script("import socket\nsocket.socket(socket.AF_INET, socket.SOCK_STREAM)\n");
    let error = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap_err();
    assert!(error.is_script_fault(), "{error}");
    assert!(error.to_string().contains("PermissionError"), "{error}");
}

#[test]
fn executed_code_sees_no_inherited_environment() {
    // The credential the application holds must not be visible to the
    // script. The variable is set in this process; the worker is started
    // with a cleared environment.
    // SAFETY: the test binary is single-threaded at this point in this test.
    unsafe { std::env::set_var("CADMARK_TEST_CREDENTIAL", "must-not-leak") };
    let (_project, script, mut worker) = project_with_script(
        "import os\nfrom build123d import *\nassert 'CADMARK_TEST_CREDENTIAL' not in os.environ, 'credential leaked'\nassert 'HOME' not in os.environ\n\nwith BuildPart() as part:\n    Box(2, 2, 2)\n",
    );
    worker.execute(&script, roomy(), &CancelFlag::new()).unwrap();
}

#[test]
fn a_cancelled_execution_stops_the_script_and_reports_cancellation() {
    let (_project, script, mut worker) =
        project_with_script("import time\nwhile True:\n    time.sleep(0.05)\n");
    let cancel = CancelFlag::new();
    let canceller = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        canceller.cancel();
    });
    let started = std::time::Instant::now();
    let error = worker.execute(&script, roomy(), &cancel).unwrap_err();
    assert!(matches!(error, WorkerError::Cancelled), "{error}");
    assert!(!error.is_script_fault());
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_missing_worker_binary_is_a_runtime_fault() {
    let project = tempfile::tempdir().unwrap();
    let script = project.path().join("part.py");
    std::fs::write(&script, BOX).unwrap();
    let mut worker = KernelWorker::new(WorkerLaunch {
        binary: project.path().join("no-such-worker"),
        project_dir: project.path().to_path_buf(),
        venv: Some(venv()),
    });
    let error = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap_err();
    assert!(matches!(error, WorkerError::Runtime(_)), "{error}");
    assert!(!error.is_script_fault());
}

/// Overwrite the project's script and return its path.
fn script_path_with(project: &tempfile::TempDir, source: &str) -> PathBuf {
    let script = project.path().join("part.py");
    std::fs::write(&script, source).unwrap();
    script
}
