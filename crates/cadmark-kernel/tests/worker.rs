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
use cadmark_core::geometry::{EdgeId, FaceId, SolidValidity, TopologyElement, VertexId};
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
/// Every solid closed and valid: what the export gate asks of a result.
fn printable(validity: &[SolidValidity]) -> bool {
    !validity.is_empty() && validity.iter().all(|solid| solid.is_printable())
}

fn roomy() -> ExecutionLimits {
    ExecutionLimits {
        wall_clock: Duration::from_secs(120),
        memory_bytes: 8 * 1024 * 1024 * 1024,
    }
}

const BOX: &str = "from build123d import *\n\nwith BuildPart() as part:\n    Box(10, 10, 10)\n";

#[test]
fn documented_modelling_recipes_build_one_closed_valid_part_each() {
    let guide = include_str!("../../../agent-docs/guides/modelling.md");
    let examples: Vec<_> = guide
        .split("```python\n")
        .skip(1)
        .map(|section| section.split("```").next().unwrap())
        .collect();
    assert_eq!(examples.len(), 2, "threaded bolt and enclosure examples");
    for (source, expected_volume) in examples.into_iter().zip([1349.995692, 5832.0]) {
        let (_project, script, mut worker) = project_with_script(source);
        let model = worker
            .execute(&script, roomy(), &CancelFlag::new())
            .expect("the published recipe executes through the application kernel");
        let parts = &model.solid().expect("solid geometry").parts;
        assert_eq!(
            parts.len(),
            1,
            "intermediate geometry must not appear as spare parts"
        );
        assert!(
            printable(&model.solid().expect("solid geometry").validity),
            "closed, valid geometry"
        );
        assert!((parts[0].summary.volume - expected_volume).abs() < 0.01);
    }
}

const SEPARATED_BOXES: &str = "from build123d import *\n\nwith BuildPart() as part:\n    Box(10, 10, 10)\n    with Locations((20, 0, 0)):\n        Box(10, 10, 10)\n";

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
        ExportFormat::Svg | ExportFormat::Dxf => {
            unreachable!("a solid round trip never goes through a drawing")
        }
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
    let solid = source.solid().expect("a solid result");
    assert!(
        printable(&solid.validity),
        "source model must be a closed solid"
    );

    let path = project
        .path()
        .join(format!("round-trip.{}", format.extension()));
    worker
        .export(&solid.file, format, &path, None, roomy())
        .unwrap_or_else(|error| panic!("{} export failed: {error}", format.label()));
    let imported = import_export(&path, format);
    assert_round_trip(format, solid.summary.volume, solid.summary.size(), imported);
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

/// A sketch-only script's kept file exports as a drawing through the
/// worker, on the sketch's own plane, and the file re-imports at the
/// sketch's size.
#[test]
fn a_sketch_exports_as_a_drawing_through_the_worker() {
    let (project, script, mut worker) = project_with_script(
        "from build123d import *\n\nwith BuildSketch(Plane.XZ) as profile:\n    RectangleRounded(30, 12, 3)\n",
    );
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();
    let sketch = model.sketch_result().expect("a sketch result");
    assert!(sketch.file.0.is_file(), "sketch kept at {:?}", sketch.file);

    let export = project.path().join("profile.svg");
    worker
        .export(
            &sketch.file,
            ExportFormat::Svg,
            &export,
            Some(sketch.profile.plane),
            roomy(),
        )
        .unwrap();
    let output = Command::new(venv().join("bin/python"))
        .arg("-c")
        .arg(
            "import json, sys\nfrom build123d import import_svg\nwires = import_svg(sys.argv[1])\nbox = wires[0].bounding_box()\nprint(json.dumps([box.size.X, box.size.Y]))",
        )
        .arg(&export)
        .output()
        .expect("the project Python runtime should launch an import reader");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let size: [f64; 2] = serde_json::from_slice(&output.stdout).unwrap();
    assert!((size[0] - 30.0).abs() < 0.2, "SVG width {}", size[0]);
    assert!((size[1] - 12.0).abs() < 0.2, "SVG height {}", size[1]);
}

#[test]
fn executes_a_script_and_exports_its_kept_model() {
    let (project, script, mut worker) = project_with_script(BOX);
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();
    assert_eq!(model.ledger.face_count(), 6);
    assert_eq!(model.solid().expect("a solid result").summary.face_count, 6);
    assert!(printable(&model.solid().expect("a solid result").validity));
    assert!(
        model.solid().expect("a solid result").file.0.is_file(),
        "model kept at {:?}",
        model.solid().expect("a solid result").file
    );

    let export = project.path().join("part.stl");
    worker
        .export(
            &model.solid().expect("a solid result").file,
            ExportFormat::Stl,
            &export,
            None,
            roomy(),
        )
        .unwrap();
    assert!(std::fs::metadata(&export).unwrap().len() > 0);
}

#[test]
fn measures_the_known_gap_between_faces_through_the_worker() {
    let (_project, script, mut worker) = project_with_script(SEPARATED_BOXES);
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();

    let measurement = worker
        .minimum_distance(
            &model.solid().expect("a solid result").file,
            TopologyElement::Face(FaceId(0)),
            TopologyElement::Face(FaceId(7)),
            roomy(),
        )
        .unwrap();

    assert!(
        (measurement.millimetres - 10.0).abs() < 1e-6,
        "{measurement:?}"
    );
}

#[test]
fn measures_picked_edge_ids_in_the_topology_map_order() {
    let (_project, script, mut worker) = project_with_script(BOX);
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();

    let edge_distance = worker
        .minimum_distance(
            &model.solid().expect("a solid result").file,
            TopologyElement::Edge(EdgeId(0)),
            TopologyElement::Edge(EdgeId(11)),
            roomy(),
        )
        .unwrap();
    assert!(
        (edge_distance.millimetres - 10.0).abs() < 1e-6,
        "{edge_distance:?}"
    );

    let error = worker
        .minimum_distance(
            &model.solid().expect("a solid result").file,
            TopologyElement::Edge(EdgeId(20)),
            TopologyElement::Edge(EdgeId(0)),
            roomy(),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("edge 20 is not in the retained model")
    );
}

#[test]
fn measures_picked_vertex_ids_in_the_topology_map_order() {
    let (_project, script, mut worker) = project_with_script(BOX);
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();

    let vertex_distance = worker
        .minimum_distance(
            &model.solid().expect("a solid result").file,
            TopologyElement::Vertex(VertexId(2)),
            TopologyElement::Vertex(VertexId(4)),
            roomy(),
        )
        .unwrap();
    assert!(
        (vertex_distance.millimetres - (200.0_f64).sqrt()).abs() < 1e-6,
        "{vertex_distance:?}"
    );
}

#[test]
fn exposes_known_circular_edge_radius_and_face_area_in_descriptors() {
    let (_project, script, mut worker) = project_with_script(
        "from build123d import *\n\nwith BuildPart() as part:\n    Cylinder(5, 10)\n",
    );
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();
    let circle = model
        .descriptors
        .edges
        .iter()
        .find(|edge| edge.curve_type == "circle")
        .expect("cylinder has circular edges");
    assert_eq!(circle.radius, Some(5.0));
    assert!(
        model
            .descriptors
            .faces
            .iter()
            .any(|face| { (face.area - std::f64::consts::PI * 25.0).abs() < 1e-6 })
    );
}

#[test]
fn execution_keeps_each_completed_part_with_its_own_measurements_and_brep() {
    let source = "from build123d import *\n\nwith BuildPart() as bracket:\n    Box(10, 10, 2)\n\nwith BuildPart() as cap:\n    with Locations((30, 0, 0)):\n        Cylinder(3, 5)\n";
    let (_project, script, mut worker) = project_with_script(source);

    let executed = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .expect("two completed parts execute through the real worker");

    let parts = &executed.solid().expect("a solid result").parts;

    assert_eq!(parts.len(), 2, "both completed builders survive discovery");
    assert_eq!(parts[0].id, 0);
    assert_eq!(parts[0].name, "bracket");
    assert_eq!(parts[1].id, 1);
    assert_eq!(parts[1].name, "cap");
    assert!(printable(&parts[0].validity));
    assert!(printable(&parts[1].validity));
    assert!((parts[0].summary.volume - 200.0).abs() < 1e-6);
    assert!((parts[1].summary.volume - (45.0 * std::f64::consts::PI)).abs() < 1e-4);
    assert_ne!(parts[0].file, parts[1].file);
    assert!(parts.iter().all(|part| part.file.0.is_file()));
}

#[test]
fn aliases_do_not_duplicate_or_hide_completed_builder_parts() {
    let source = "from build123d import *\n\nwith BuildPart() as bracket:\n    Box(10, 10, 2)\n\nwith BuildPart() as cap:\n    with Locations((30, 0, 0)):\n        Cylinder(3, 5)\nresult = cap.part\n";
    let (_project, script, mut worker) = project_with_script(source);
    let executed = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();

    let parts = &executed.solid().expect("a solid result").parts;
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "bracket");
    assert_eq!(parts[1].name, "result");
}

#[test]
fn independent_algebra_mode_solids_are_separate_parts() {
    let source = "from build123d import *\n\nbracket = Part() + Box(10, 10, 2)\ncap = Part() + Pos(30, 0, 0) * Cylinder(3, 5)\n";
    let (_project, script, mut worker) = project_with_script(source);
    let executed = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .expect("independent algebra-mode solids execute through the real worker");

    let parts = &executed.solid().expect("a solid result").parts;

    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "bracket");
    assert_eq!(parts[1].name, "cap");
    assert_ne!(parts[0].file, parts[1].file);
}

#[test]
fn mixed_builder_and_algebra_mode_solids_are_separate_parts() {
    let source = "from build123d import *\n\nwith BuildPart() as bracket:\n    Box(10, 10, 2)\ncap = Part() + Pos(30, 0, 0) * Cylinder(3, 5)\n";
    let (_project, script, mut worker) = project_with_script(source);
    let executed = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .expect("mixed builder and algebra-mode solids execute through the real worker");

    let parts = &executed.solid().expect("a solid result").parts;

    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "bracket");
    assert_eq!(parts[1].name, "cap");
    assert_ne!(parts[0].file, parts[1].file);
}

#[test]
fn rebinding_one_name_keeps_one_part_with_the_base_volume() {
    let source = "from build123d import *\n\npart = Part() + Box(20, 20, 5)\npart = part - Pos(0, 0, 0) * Cylinder(3, 20)\n";
    let (_project, script, mut worker) = project_with_script(source);
    let executed = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();

    let parts = &executed.solid().expect("a solid result").parts;

    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].name, "part");
    assert!((parts[0].summary.volume - 1_858.628_330_588_459).abs() < 1e-6);
}

#[test]
fn a_part_used_for_relative_placement_remains_a_second_part() {
    let source = "from build123d import *\n\nbracket = Part() + Box(10, 10, 2)\ncap = Part() + Pos(0, 0, bracket.bounding_box().size.Z) * Cylinder(3, 5)\n";
    let (_project, script, mut worker) = project_with_script(source);
    let executed = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();

    let parts = &executed.solid().expect("a solid result").parts;

    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "bracket");
    assert_eq!(parts[1].name, "cap");
}

#[test]
fn a_labelled_shape_is_named_by_its_label_under_any_binding() {
    // A builder's part and an algebra-mode solid, each given a label:
    // the label names the part, and a later alias does not rename it.
    let source = "from build123d import *\n\nwith BuildPart() as bp:\n    Box(10, 10, 2)\nbp.part.label = \"base plate\"\n\ncap = Part() + Pos(30, 0, 0) * Cylinder(3, 5)\ncap.label = \"lid\"\nresult = cap\n";
    let (_project, script, mut worker) = project_with_script(source);
    let executed = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();

    let parts = &executed.solid().expect("a solid result").parts;
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "base plate");
    assert_eq!(parts[1].name, "lid");
}

#[test]
fn parts_sharing_a_name_are_numbered_apart() {
    // Two labels the same, and a third part whose binding repeats the
    // label: every part still has a name of its own, in script order.
    let source = "from build123d import *\n\na = Part() + Box(10, 10, 2)\na.label = \"lid\"\nb = Part() + Pos(30, 0, 0) * Box(10, 10, 2)\nb.label = \"lid\"\nlid = Part() + Pos(60, 0, 0) * Box(10, 10, 2)\n";
    let (_project, script, mut worker) = project_with_script(source);
    let executed = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();

    let parts = &executed.solid().expect("a solid result").parts;
    let names: Vec<&str> = parts.iter().map(|part| part.name.as_str()).collect();
    assert_eq!(names, ["lid", "lid (2)", "lid (3)"]);
}

#[test]
fn an_alias_of_one_shape_is_one_part() {
    let source = "from build123d import *\n\npart = Part() + Box(10, 10, 2)\nalias = part\n";
    let (_project, script, mut worker) = project_with_script(source);
    let executed = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap();

    let parts = &executed.solid().expect("a solid result").parts;

    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].name, "alias");
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
        .export(
            &model.solid().expect("a solid result").file,
            ExportFormat::Step,
            &export,
            None,
            roomy(),
        )
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

// ── Scratch snippets and printed output ───────────────────────────────

const BOX_THAT_PRINTS: &str = "from build123d import *\n\nprint('building the box')\nwith BuildPart() as part:\n    Box(10, 10, 10)\nprint('volume', round(part.part.volume))\n";

#[test]
fn what_a_script_prints_comes_back_with_its_model() {
    let (_project, script, mut worker) = project_with_script(BOX_THAT_PRINTS);
    let model = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .expect("the box builds");
    assert_eq!(model.printed, "building the box\nvolume 1000\n");
}

#[test]
fn a_script_that_fails_carries_what_it_printed_first() {
    let (_project, script, mut worker) = project_with_script(
        "from build123d import *\nprint('about to fail')\nraise ValueError('deliberate')\n",
    );
    let error = worker
        .execute(&script, roomy(), &CancelFlag::new())
        .unwrap_err();
    assert!(error.is_script_fault(), "{error}");
    let message = error.to_string();
    assert!(message.contains("ValueError: deliberate"), "{message}");
    assert!(
        message.contains("Printed before it failed:\nabout to fail"),
        "{message}"
    );
}

#[test]
fn a_snippet_after_the_script_sees_its_bindings_and_reports_value_and_prints() {
    let (_project, script, mut worker) = project_with_script(BOX_THAT_PRINTS);
    let outcome = worker
        .run_snippet(
            Some(&script),
            "faces = part.part.faces()\nprint('faces', len(faces))\nround(part.part.volume)",
            roomy(),
            &CancelFlag::new(),
        )
        .expect("the snippet runs");
    assert_eq!(outcome.error, None, "{outcome:?}");
    assert_eq!(outcome.value.as_deref(), Some("1000"));
    // The script's own prints come first, then the snippet's.
    assert_eq!(outcome.printed, "building the box\nvolume 1000\nfaces 6\n");
}

#[test]
fn a_standalone_snippet_runs_without_the_script_and_a_statement_has_no_value() {
    let (_project, _script, mut worker) = project_with_script(BOX);
    let outcome = worker
        .run_snippet(
            None,
            "import math\nmath.hypot(3, 4)",
            roomy(),
            &CancelFlag::new(),
        )
        .expect("the snippet runs");
    assert_eq!(outcome.value.as_deref(), Some("5.0"));
    assert_eq!(outcome.error, None);
    // A trailing assignment is a statement: nothing to report as a value,
    // and the script's bindings are absent because it did not run.
    let outcome = worker
        .run_snippet(None, "x = 'part' in dir()", roomy(), &CancelFlag::new())
        .expect("the snippet runs");
    assert_eq!(outcome.value, None);
    let outcome = worker
        .run_snippet(None, "x", roomy(), &CancelFlag::new())
        .expect("the snippet runs");
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("NameError")),
        "each snippet starts from an empty namespace: {outcome:?}"
    );
}

#[test]
fn a_snippet_that_raises_returns_the_traceback_as_its_outcome() {
    let (_project, script, mut worker) = project_with_script(BOX);
    let outcome = worker
        .run_snippet(
            Some(&script),
            "edges = part.part.edges()\nedges[99]",
            roomy(),
            &CancelFlag::new(),
        )
        .expect("a raising snippet is still an outcome");
    let error = outcome.error.expect("the traceback");
    assert!(error.contains("line 2"), "{error}");
    assert!(error.contains("IndexError"), "{error}");
    assert_eq!(outcome.value, None);
}

#[test]
fn a_broken_script_is_reported_before_the_snippet_runs() {
    let (_project, script, mut worker) =
        project_with_script("from build123d import *\nBox(10, 10, 'ten')\n");
    let outcome = worker
        .run_snippet(Some(&script), "1 + 1", roomy(), &CancelFlag::new())
        .expect("a failing script is an outcome for a snippet");
    let error = outcome.error.expect("the script's traceback");
    assert!(
        error.starts_with("The script itself failed before the snippet ran:"),
        "{error}"
    );
    assert!(error.contains("line 2"), "{error}");
    assert_eq!(outcome.value, None);
}

#[test]
fn a_snippet_runs_under_the_wall_clock_ceiling() {
    let (_project, script, mut worker) = project_with_script(BOX);
    let limits = ExecutionLimits {
        wall_clock: Duration::from_secs(2),
        ..roomy()
    };
    let error = worker
        .run_snippet(
            Some(&script),
            "while True:\n    pass",
            limits,
            &CancelFlag::new(),
        )
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
    // The worker is replaced transparently: the next request is served.
    let outcome = worker
        .run_snippet(None, "2 + 2", roomy(), &CancelFlag::new())
        .expect("a fresh worker serves the next snippet");
    assert_eq!(outcome.value.as_deref(), Some("4"));
}
