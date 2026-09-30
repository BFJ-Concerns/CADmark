// The wire contract between the application and the kernel worker process:
// one JSON request per line on the worker's stdin, one JSON reply per line
// on its stdout. Everything here is plain serialisable data; the worker's
// process boundary is where kernel-specific types stop.

use std::path::PathBuf;

use cadmark_core::export::ExportFormat;
use cadmark_core::geometry::{
    GeometryDescriptors, MinimumDistance, ModelSummary, PartMeasurements, SolidValidity,
    TopologyElement,
};
use cadmark_core::ledger::ProvenanceLedger;
use cadmark_core::mesh::TessellatedMesh;
use cadmark_core::sketch::{SketchPlane, SketchProfile};
use cadmark_core::sketch_lineage::SketchLineageLedger;
use serde::{Deserialize, Serialize};

/// A request the application sends to the worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerRequest {
    /// Execute the script at `script_path` (inside the project folder) and
    /// keep its model as a file in the worker's scratch directory.
    Execute { script_path: PathBuf },
    /// Write a retained model to `path` in `format`. A drawing format
    /// flattens the model onto `plane`, the plane a sketch was drawn on;
    /// solid formats ignore it.
    Export {
        model: ModelFile,
        format: ExportFormat,
        path: PathBuf,
        plane: Option<SketchPlane>,
    },
    /// Measure the closest separation between two elements of a retained model.
    MinimumDistance {
        model: ModelFile,
        first: TopologyElement,
        second: TopologyElement,
    },
    /// Run a scratch snippet of Python for its printed output and the
    /// value of its last expression, without keeping a model. With a
    /// `script_path` the script runs first and the snippet sees its
    /// namespace; without one the snippet runs in an empty namespace.
    RunSnippet {
        script_path: Option<PathBuf>,
        code: String,
    },
}

/// What a scratch snippet produced. A snippet that raises is still an
/// outcome — the traceback is what the caller wanted to read — so `error`
/// is a field rather than a failed reply.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SnippetOutcome {
    /// Everything the script and the snippet printed to stdout, in order,
    /// cut at `PRINTED_OUTPUT_LIMIT` characters.
    pub printed: String,
    /// The `repr` of the snippet's final expression, when it ended in one
    /// whose value was not `None`.
    pub value: Option<String>,
    /// The formatted traceback when the script or the snippet raised.
    pub error: Option<String>,
}

/// How much printed output one execution or snippet keeps. Enough for a
/// table of measurements or a dump of a namespace; a runaway loop printing
/// in every iteration is cut rather than carried across the boundary.
pub const PRINTED_OUTPUT_LIMIT: usize = 20_000;

/// A model the worker kept after a successful execution: a BREP file in
/// its scratch directory, re-read for export. Cloning shares the file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelFile(pub PathBuf);

/// A solid result: measured, checked for validity, and kept on disk so it
/// can be exported and measured again without re-running the script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolidResult {
    /// Whole-model measurements.
    pub summary: ModelSummary,
    /// One entry per solid in the model, in the kernel's traversal order.
    pub validity: Vec<SolidValidity>,
    /// The model, retained for export.
    pub file: ModelFile,
    /// Every independently completed part the script produced, in source
    /// binding order. `id` is stable for one execution and `name` preserves
    /// the binding the script author can recognise.
    pub parts: Vec<ExecutedPart>,
}

/// One independently selectable and exportable solid from an execution.
/// All fields are plain data because this crosses the worker boundary.
/// Face, edge and vertex numbering restarts within each part, so a part
/// carries its own ledger and descriptor tables rather than sharing the
/// model's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutedPart {
    pub id: u32,
    pub name: String,
    pub mesh: TessellatedMesh,
    pub ledger: ProvenanceLedger,
    pub sketch_lineage: SketchLineageLedger,
    pub descriptors: GeometryDescriptors,
    pub summary: ModelSummary,
    pub validity: Vec<SolidValidity>,
    /// The part's own BREP retained in worker scratch for export.
    pub file: ModelFile,
}

impl ExecutedPart {
    /// A part named by the script and kept at `file`, with empty geometry,
    /// ledgers, measurements and validity: the base a test builds on,
    /// naming only what it reads.
    pub fn new(id: u32, name: impl Into<String>, file: ModelFile) -> Self {
        Self {
            id,
            name: name.into(),
            mesh: TessellatedMesh::default(),
            ledger: ProvenanceLedger::new(),
            sketch_lineage: SketchLineageLedger::new(),
            descriptors: GeometryDescriptors::default(),
            summary: ModelSummary::default(),
            validity: Vec::new(),
            file,
        }
    }

    /// The part's measurements under the name the script gave it.
    pub fn measurements(&self) -> PartMeasurements {
        PartMeasurements::new(self.name.clone(), self.summary.clone())
    }
}

impl SolidResult {
    /// A solid measured as `summary` and kept at `file`, with no validity
    /// entries and no parts yet.
    pub fn new(summary: ModelSummary, file: ModelFile) -> Self {
        Self {
            summary,
            validity: Vec::new(),
            file,
            parts: Vec::new(),
        }
    }

    /// Every part's measurements, in source binding order.
    pub fn part_measurements(&self) -> Vec<PartMeasurements> {
        self.parts.iter().map(ExecutedPart::measurements).collect()
    }
}

/// A sketch result: the profile drawn, kept on disk so it can be exported
/// as a drawing without re-running the script. It has no volume to
/// summarise and no solid to check, so it carries neither rather than
/// zeroed stand-ins that would read as a degenerate solid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchResult {
    pub profile: SketchProfile,
    /// The placed sketch shape, retained for export.
    pub file: ModelFile,
}

impl SketchResult {
    pub fn new(profile: SketchProfile, file: ModelFile) -> Self {
        Self { profile, file }
    }
}

/// What kind of result a script reached: a solid, or a profile it has
/// drawn and not yet made a solid of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum ModelForm {
    Solid(SolidResult),
    Sketch(SketchResult),
}

/// Everything the application keeps from a successful execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutedModel {
    /// Tessellated geometry for the renderer. A sketch-only result has no
    /// triangles: its geometry is in the profile its form carries.
    pub mesh: TessellatedMesh,
    /// Construction-time provenance for every element of the mesh.
    pub ledger: ProvenanceLedger,
    /// Which sketch curve drew each element, or the stated reason none
    /// can be named: keyed by face, edge and vertex for a solid, and by
    /// region, curve and corner for a drawn profile. Rebuilt with the
    /// ledger on every execution.
    pub sketch_lineage: SketchLineageLedger,
    /// Measured geometry of every element, in ledger order.
    pub descriptors: GeometryDescriptors,
    /// The result itself, in the shape its kind actually has.
    pub form: ModelForm,
    /// What the script printed to stdout while it ran, cut at
    /// `PRINTED_OUTPUT_LIMIT` characters, so a `print` the author put in
    /// to check a value reaches them rather than the worker's log.
    #[serde(default)]
    pub printed: String,
}

impl ExecutedModel {
    /// A model in the given form with empty geometry, ledgers, descriptors
    /// and printed output: what a result is before the kernel fills it in,
    /// and the base a test builds on, naming only what it reads.
    pub fn of(form: ModelForm) -> Self {
        Self {
            mesh: TessellatedMesh::default(),
            ledger: ProvenanceLedger::new(),
            sketch_lineage: SketchLineageLedger::new(),
            descriptors: GeometryDescriptors::default(),
            form,
            printed: String::new(),
        }
    }

    /// The solid this execution produced, or `None` when it reached only a
    /// sketch.
    pub fn solid(&self) -> Option<&SolidResult> {
        match &self.form {
            ModelForm::Solid(solid) => Some(solid),
            ModelForm::Sketch(_) => None,
        }
    }

    /// The profile this execution drew, when no solid came of it yet.
    pub fn sketch(&self) -> Option<&SketchProfile> {
        self.sketch_result().map(|sketch| &sketch.profile)
    }

    /// The sketch this execution drew and kept, when no solid came of it.
    pub fn sketch_result(&self) -> Option<&SketchResult> {
        match &self.form {
            ModelForm::Sketch(sketch) => Some(sketch),
            ModelForm::Solid(_) => None,
        }
    }
}

/// Whether every solid is closed and valid — the question the export gate
/// asks of a model, stated in one word for the kernel's own tests. The
/// application decides export from the per-solid `validity` directly.
#[cfg(test)]
impl ExecutedModel {
    pub(crate) fn is_printable(&self) -> bool {
        self.solid().is_some_and(|solid| {
            !solid.validity.is_empty() && solid.validity.iter().all(|solid| solid.is_printable())
        })
    }
}

/// Why the worker could not complete a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum WorkerFailure {
    /// The script itself failed: a traceback, a syntax error, or an output
    /// shape the kernel does not accept. The AI can act on the message.
    Script { message: String },
    /// CADmark's own runtime failed. Not the AI's to repair.
    Runtime { message: String },
}

impl WorkerFailure {
    pub fn message(&self) -> &str {
        match self {
            Self::Script { message } | Self::Runtime { message } => message,
        }
    }
}

/// The reply to one request. The executed model is boxed: it dwarfs the
/// other variants and every reply is moved once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WorkerReply {
    Executed(Box<ExecutedModel>),
    Exported,
    MinimumDistance(MinimumDistance),
    SnippetRan(SnippetOutcome),
    Failed(WorkerFailure),
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadmark_core::geometry::{FaceId, MinimumDistance, ModelSummary, TopologyElement};
    use cadmark_core::ledger::LedgerValue;

    #[test]
    fn requests_and_replies_round_trip_as_single_json_lines() {
        let request = WorkerRequest::Execute {
            script_path: PathBuf::from("/project/part.py"),
        };
        let line = serde_json::to_string(&request).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(
            serde_json::from_str::<WorkerRequest>(&line).unwrap(),
            request
        );

        let snippet = WorkerRequest::RunSnippet {
            script_path: Some(PathBuf::from("/project/part.py")),
            code: "part.part.volume".to_string(),
        };
        let line = serde_json::to_string(&snippet).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(
            serde_json::from_str::<WorkerRequest>(&line).unwrap(),
            snippet
        );
        let ran = WorkerReply::SnippetRan(SnippetOutcome {
            printed: "checking\n".to_string(),
            value: Some("1000.0".to_string()),
            error: None,
        });
        let line = serde_json::to_string(&ran).unwrap();
        assert_eq!(serde_json::from_str::<WorkerReply>(&line).unwrap(), ran);

        let mut ledger = ProvenanceLedger::new();
        ledger
            .record_face(FaceId(0), LedgerValue::Untraced)
            .unwrap();
        let reply = WorkerReply::Executed(Box::new(ExecutedModel {
            ledger,
            ..ExecutedModel::of(ModelForm::Solid(SolidResult {
                validity: vec![SolidValidity {
                    closed: true,
                    valid: true,
                    ..Default::default()
                }],
                ..SolidResult::new(
                    ModelSummary {
                        volume: 1.0,
                        bounds_max: [1.0; 3],
                        face_count: 1,
                        ..Default::default()
                    },
                    ModelFile(PathBuf::from("/scratch/model-1.brep")),
                )
            }))
        }));
        let line = serde_json::to_string(&reply).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<WorkerReply>(&line).unwrap(), reply);

        let request = WorkerRequest::Export {
            model: ModelFile(PathBuf::from("/scratch/model-1.brep")),
            format: ExportFormat::Svg,
            path: PathBuf::from("/project/part.svg"),
            plane: Some(SketchPlane::default()),
        };
        let line = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<WorkerRequest>(&line).unwrap(),
            request
        );

        let request = WorkerRequest::MinimumDistance {
            model: ModelFile(PathBuf::from("/scratch/model-1.brep")),
            first: TopologyElement::Face(FaceId(0)),
            second: TopologyElement::Face(FaceId(6)),
        };
        let line = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<WorkerRequest>(&line).unwrap(),
            request
        );
        let reply = WorkerReply::MinimumDistance(MinimumDistance { millimetres: 20.0 });
        let line = serde_json::to_string(&reply).unwrap();
        assert_eq!(serde_json::from_str::<WorkerReply>(&line).unwrap(), reply);
    }

    #[test]
    fn printable_needs_every_solid_closed_and_valid() {
        let model = |validity: Vec<SolidValidity>| {
            ExecutedModel::of(ModelForm::Solid(SolidResult {
                validity,
                ..SolidResult::new(ModelSummary::default(), ModelFile(PathBuf::new()))
            }))
        };
        let good = SolidValidity {
            closed: true,
            valid: true,
            ..Default::default()
        };
        let open = SolidValidity::default();
        assert!(model(vec![good, good]).is_printable());
        assert!(!model(vec![good, open]).is_printable());
        assert!(!model(Vec::new()).is_printable());
    }

    #[test]
    fn a_sketch_result_crosses_the_boundary_carrying_no_solid() {
        use cadmark_core::sketch::{SketchCorner, SketchCurve, SketchProfile};

        let executed = ExecutedModel::of(ModelForm::Sketch(SketchResult::new(
            SketchProfile {
                curves: vec![SketchCurve {
                    points: vec![[0.0; 3], [1.0, 0.0, 0.0]],
                    curve_type: "line".to_string(),
                    length: 1.0,
                    ..Default::default()
                }],
                corners: vec![SketchCorner::default()],
                ..Default::default()
            },
            ModelFile(PathBuf::from("/scratch/model-2.brep")),
        )));

        // Nothing about a sketch may read as a solid: no solid export gate
        // opens, and the solid accessor states its absence rather than a
        // zero; the sketch itself is kept for a drawing export.
        assert!(!executed.is_printable());
        assert!(executed.solid().is_none());
        assert_eq!(executed.sketch().map(|s| s.curves.len()), Some(1));
        assert_eq!(
            executed.sketch_result().map(|s| s.file.0.as_path()),
            Some(std::path::Path::new("/scratch/model-2.brep"))
        );

        let reply = WorkerReply::Executed(Box::new(executed));
        let line = serde_json::to_string(&reply).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<WorkerReply>(&line).unwrap(), reply);
    }
}
