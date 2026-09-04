// The kernel worker, driven as the application drives it: the real
// `cadmark-kernel-worker` binary, a real project folder, the real Python
// runtime. These are the "kernel, called as the app calls it" rung's
// proofs of the execution boundary: limits trip, confinement holds, a
// cancel stops the script, and a killed worker is replaced.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cadmark_core::cancellation::CancelFlag;
use cadmark_core::export::ExportFormat;
use cadmark_core::limits::{ExecutionLimits, LimitHit};
use cadmark_kernel::worker::{KernelWorker, WorkerError, WorkerLaunch};
use serde::Deserialize;

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

// Curvature makes mesh faceting observable: unlike a box, a cylinder's mesh
// round trip loses a small, measurable amount of volume.
const CURVED_PART: &str =
    "from build123d import *\n\nwith BuildPart() as part:\n    Cylinder(10, 10)\n";

const STEP_RELATIVE_TOLERANCE: f64 = 0.000_1;
const MESH_RELATIVE_TOLERANCE: f64 = 0.01;

#[derive(Debug, Deserialize)]
struct ImportedModel {
    volume: f64,
    size: [f64; 3],
    closed: bool,
    valid: Option<bool>,
}

/// Re-import one written file in a fresh Python process, outside the worker.
/// STEP uses build123d's B-rep importer, STL its Lib3MF mesh reader, and 3MF
/// is parsed independently with the Python standard library's ZIP and XML APIs.
fn import_export(path: &Path, format: ExportFormat) -> ImportedModel {
    let reader = r#"
import json
import sys

path, format_name = sys.argv[1:]
if format_name == "step":
    from build123d import import_step
    shapes = [import_step(path)]
elif format_name == "stl":
    from build123d import Mesher
    shapes = Mesher().read(path)
if format_name in ("step", "stl"):
    assert len(shapes) == 1, f"expected one imported shape, got {len(shapes)}"
    shape = shapes[0]
    box = shape.bounding_box()
    result = {
        "volume": shape.volume,
        "size": list(box.size),
        "closed": shape.is_manifold,
        "valid": shape.is_valid,
    }
elif format_name == "3mf":
    import xml.etree.ElementTree as ET
    from collections import Counter
    from zipfile import ZipFile

    with ZipFile(path) as archive:
        model_name = next(name for name in archive.namelist() if name.endswith(".model"))
        root = ET.fromstring(archive.read(model_name))
    meshes = root.findall(".//{*}mesh")
    assert len(meshes) == 1, f"expected one 3MF mesh, got {len(meshes)}"
    mesh = meshes[0]
    vertices = [
        tuple(float(vertex.attrib[axis]) for axis in ("x", "y", "z"))
        for vertex in mesh.findall("./{*}vertices/{*}vertex")
    ]
    triangles = [
        tuple(int(triangle.attrib[index]) for index in ("v1", "v2", "v3"))
        for triangle in mesh.findall("./{*}triangles/{*}triangle")
    ]
    assert vertices and triangles, "3MF mesh must contain vertices and triangles"
    assert all(0 <= index < len(vertices) for triangle in triangles for index in triangle)
    edges = Counter(
        tuple(sorted((triangle[index], triangle[(index + 1) % 3])))
        for triangle in triangles
        for index in range(3)
    )
    volume = abs(sum(
        vertices[a][0] * (vertices[b][1] * vertices[c][2] - vertices[b][2] * vertices[c][1])
        + vertices[a][1] * (vertices[b][2] * vertices[c][0] - vertices[b][0] * vertices[c][2])
        + vertices[a][2] * (vertices[b][0] * vertices[c][1] - vertices[b][1] * vertices[c][0])
        for a, b, c in triangles
    ) / 6.0)
    result = {
        "volume": volume,
        "size": [max(vertex[axis] for vertex in vertices) - min(vertex[axis] for vertex in vertices) for axis in range(3)],
        "closed": all(count == 2 for count in edges.values()),
    }
else:
    raise ValueError(f"unknown format {format_name}")

print(json.dumps(result))
"#;
    let output = Command::new(venv().join("bin/python"))
        .arg("-c")
        .arg(reader)
        .arg(path)
        .arg(format.extension())
        .output()
        .expect("the project Python runtime should launch an import reader");
    assert!(
        output.status.success(),
        "the {} reader failed: {}",
        format.label(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "the {} reader did not return measurements: {error}; stdout: {}",
            format.label(),
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn relative_difference(actual: f64, expected: f64) -> f64 {
    (actual - expected).abs() / expected.abs()
}

fn assert_round_trip(
    format: ExportFormat,
    source_volume: f64,
    source_size: [f64; 3],
    imported: ImportedModel,
) {
    let tolerance = match format {
        ExportFormat::Step => STEP_RELATIVE_TOLERANCE,
        ExportFormat::Stl | ExportFormat::ThreeMf => MESH_RELATIVE_TOLERANCE,
    };
    assert!(
        imported.closed,
        "{} import was not a closed solid",
        format.label()
    );
    if let Some(valid) = imported.valid {
        assert!(valid, "{} import was invalid", format.label());
    }
    assert!(
        relative_difference(imported.volume, source_volume) <= tolerance,
        "{} volume {} differed from source {} by more than {:.2}%",
        format.label(),
        imported.volume,
        source_volume,
        tolerance * 100.0,
    );
    for (axis, (actual, expected)) in imported.size.iter().zip(source_size).enumerate() {
        assert!(
            relative_difference(*actual, expected) <= tolerance,
            "{} size on axis {axis} was {actual}, expected {expected} within {:.2}%",
            format.label(),
            tolerance * 100.0,
        );
    }
}

fn assert_worker_export_round_trip(format: ExportFormat) {
    let (project, script, mut worker) = project_with_script(CURVED_PART);
    let source = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();
    assert!(source.is_printable(), "source model must be a closed solid");

    let path = project
        .path()
        .join(format!("round-trip.{}", format.extension()));
    worker
        .export(&source.model, format, &path, roomy())
        .unwrap_or_else(|error| panic!("{} export failed: {error}", format.label()));
    let imported = import_export(&path, format);
    assert_round_trip(
        format,
        source.summary.volume,
        source.summary.size(),
        imported,
    );
}

#[test]
fn step_export_round_trips_as_a_closed_solid_at_its_original_scale() {
    assert_worker_export_round_trip(ExportFormat::Step);
}

#[test]
fn stl_export_round_trips_as_a_closed_solid_at_its_original_scale() {
    assert_worker_export_round_trip(ExportFormat::Stl);
}

#[test]
fn three_mf_export_round_trips_as_a_closed_solid_at_its_original_scale() {
    assert_worker_export_round_trip(ExportFormat::ThreeMf);
}

#[test]
fn executes_a_script_and_exports_its_kept_model() {
    let (project, script, mut worker) = project_with_script(BOX);
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();
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
fn a_script_that_prints_does_not_corrupt_the_protocol() {
    // print() goes to the worker's stderr, never into the reply stream.
    let (_project, script, mut worker) = project_with_script(
        "from build123d import *\nprint('{\"outcome\": \"exported\"}')\nprint('hello from the script')\n\nwith BuildPart() as part:\n    Box(3, 3, 3)\n",
    );
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();
    assert_eq!(model.ledger.face_count(), 6);
}

#[test]
fn a_model_kept_before_a_killed_worker_still_exports() {
    // A limit hit kills the process but not the turn; the model an
    // earlier execution kept must survive the replacement.
    let (project, script, mut worker) = project_with_script(BOX);
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();
    let runaway = script_path_with(&project, "import time\nwhile True:\n    time.sleep(0.05)\n");
    let limits = ExecutionLimits {
        wall_clock: Duration::from_secs(1),
        ..roomy()
    };
    assert!(matches!(
        worker.execute(&runaway, limits, &CancelFlag::new()),
        Err(WorkerError::Limit { .. })
    ));
    let export = project.path().join("kept.step");
    worker
        .export(&model.model, ExportFormat::Step, &export, roomy())
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
        .execute(
            &script_path_with(&_project, BOX),
            roomy(),
            &CancelFlag::new(),
        )
        .unwrap();
    assert_eq!(model.ledger.face_count(), 6);
}

#[test]
fn a_syntax_error_is_a_script_fault_the_ai_can_fix() {
    let (_project, script, mut worker) = project_with_script(
        "from build123d import *\n\nwith BuildPart() as part\n    Box(1, 1, 1)\n",
    );
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
        .execute(
            &script_path_with(&_project, BOX),
            roomy(),
            &CancelFlag::new(),
        )
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
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();
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
    // The application's environment holds the provider credential among
    // much else. The worker starts with an empty environment, so nothing
    // the test process has — HOME, PATH, RUST_LOG — is visible to the
    // script. (The credential itself is never set in a test process;
    // these variables stand in for it.)
    assert!(
        std::env::var_os("HOME").is_some(),
        "the test process has an environment to leak"
    );
    let (_project, script, mut worker) = project_with_script(
        "import os\nfrom build123d import *\nfor name in ('HOME', 'PATH', 'RUST_LOG', 'USER'):\n    assert name not in os.environ, name + ' leaked'\n\nwith BuildPart() as part:\n    Box(2, 2, 2)\n",
    );
    worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();
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
