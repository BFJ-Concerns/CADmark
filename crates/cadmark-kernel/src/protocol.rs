// The wire contract between the application and the kernel worker process:
// one JSON request per line on the worker's stdin, one JSON reply per line
// on its stdout. Everything here is plain serialisable data; the worker's
// process boundary is where kernel-specific types stop.

use std::path::PathBuf;

use cadmark_core::export::ExportFormat;
use cadmark_core::geometry::{
    GeometryDescriptors, MinimumDistance, ModelSummary, SolidValidity, TopologyElement,
};
use cadmark_core::ledger::ProvenanceLedger;
use cadmark_core::mesh::TessellatedMesh;
use cadmark_core::sketch::SketchProfile;
use serde::{Deserialize, Serialize};

/// A request the application sends to the worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerRequest {
    /// Execute the script at `script_path` (inside the project folder) and
    /// keep its model as a file in the worker's scratch directory.
    Execute { script_path: PathBuf },
    /// Write a retained model to `path` in `format`.
    Export {
        model: ModelFile,
        format: ExportFormat,
        path: PathBuf,
    },
    /// Measure the closest separation between two elements of a retained model.
    MinimumDistance {
        model: ModelFile,
        first: TopologyElement,
        second: TopologyElement,
    },
}

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
    pub descriptors: GeometryDescriptors,
    pub summary: ModelSummary,
    pub validity: Vec<SolidValidity>,
    /// The part's own BREP retained in worker scratch for export.
    pub file: ModelFile,
}

impl ExecutedPart {
    pub fn is_printable(&self) -> bool {
        solids_are_printable(&self.validity)
    }
}

/// What kind of result a script reached. A script that has drawn a profile
/// and not yet made a solid of it has no volume to summarise, no solid to
/// check, and nothing to export, so it carries none of those rather than
/// zeroed stand-ins that would read as a degenerate solid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum ModelForm {
    Solid(SolidResult),
    Sketch(SketchProfile),
}

/// Everything the application keeps from a successful execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutedModel {
    /// Tessellated geometry for the renderer. A sketch-only result has no
    /// triangles: its geometry is in the profile its form carries.
    pub mesh: TessellatedMesh,
    /// Construction-time provenance for every element of the mesh.
    pub ledger: ProvenanceLedger,
    /// Measured geometry of every element, in ledger order.
    pub descriptors: GeometryDescriptors,
    /// The result itself, in the shape its kind actually has.
    pub form: ModelForm,
}

impl ExecutedModel {
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
        match &self.form {
            ModelForm::Sketch(sketch) => Some(sketch),
            ModelForm::Solid(_) => None,
        }
    }

    /// Whether every solid is closed and valid — what "print-ready" means
    /// for the export gate. A sketch is never printable.
    pub fn is_printable(&self) -> bool {
        self.solid()
            .is_some_and(|solid| solids_are_printable(&solid.validity))
    }
}

fn solids_are_printable(validity: &[SolidValidity]) -> bool {
    !validity.is_empty() && validity.iter().all(|solid| solid.is_printable())
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

        let mut ledger = ProvenanceLedger::new();
        ledger
            .record_face(FaceId(0), LedgerValue::Untraced)
            .unwrap();
        let reply = WorkerReply::Executed(Box::new(ExecutedModel {
            mesh: TessellatedMesh::default(),
            ledger,
            descriptors: GeometryDescriptors::default(),
            form: ModelForm::Solid(SolidResult {
                summary: ModelSummary {
                    volume: 1.0,
                    bounds_min: [0.0; 3],
                    bounds_max: [1.0; 3],
                    face_count: 1,
                    edge_count: 0,
                    vertex_count: 0,
                },
                validity: vec![SolidValidity {
                    closed: true,
                    valid: true,
                }],
                file: ModelFile(PathBuf::from("/scratch/model-1.brep")),
                parts: Vec::new(),
            }),
        }));
        let line = serde_json::to_string(&reply).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<WorkerReply>(&line).unwrap(), reply);

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
        let model = |validity: Vec<SolidValidity>| ExecutedModel {
            mesh: TessellatedMesh::default(),
            ledger: ProvenanceLedger::new(),
            descriptors: GeometryDescriptors::default(),
            form: ModelForm::Solid(SolidResult {
                summary: ModelSummary {
                    volume: 0.0,
                    bounds_min: [0.0; 3],
                    bounds_max: [0.0; 3],
                    face_count: 0,
                    edge_count: 0,
                    vertex_count: 0,
                },
                validity,
                file: ModelFile(PathBuf::new()),
                parts: Vec::new(),
            }),
        };
        let good = SolidValidity {
            closed: true,
            valid: true,
        };
        let open = SolidValidity {
            closed: false,
            valid: false,
        };
        assert!(model(vec![good, good]).is_printable());
        assert!(!model(vec![good, open]).is_printable());
        assert!(!model(Vec::new()).is_printable());
    }

    #[test]
    fn a_sketch_result_crosses_the_boundary_carrying_no_solid() {
        use cadmark_core::sketch::{SketchCorner, SketchCurve, SketchPlane, SketchProfile};

        let executed = ExecutedModel {
            mesh: TessellatedMesh::default(),
            ledger: ProvenanceLedger::new(),
            descriptors: GeometryDescriptors::default(),
            form: ModelForm::Sketch(SketchProfile {
                plane: SketchPlane::default(),
                curves: vec![SketchCurve {
                    curve_id: 0,
                    points: vec![[0.0; 3], [1.0, 0.0, 0.0]],
                }],
                corners: vec![SketchCorner {
                    corner_id: 0,
                    position: [0.0; 3],
                }],
                regions: Vec::new(),
            }),
        };

        // Nothing about a sketch may read as a solid: no export gate opens,
        // and the solid accessor states its absence rather than a zero.
        assert!(!executed.is_printable());
        assert!(executed.solid().is_none());
        assert_eq!(executed.sketch().map(|s| s.curves.len()), Some(1));

        let reply = WorkerReply::Executed(Box::new(executed));
        let line = serde_json::to_string(&reply).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<WorkerReply>(&line).unwrap(), reply);
    }
}
