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

/// Everything the application keeps from a successful execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutedModel {
    /// Tessellated geometry for the renderer.
    pub mesh: TessellatedMesh,
    /// Construction-time provenance for every element of the mesh.
    pub ledger: ProvenanceLedger,
    /// Measured geometry of every element, in ledger order.
    pub descriptors: GeometryDescriptors,
    /// Whole-model measurements.
    pub summary: ModelSummary,
    /// One entry per solid in the model, in the kernel's traversal order.
    /// A model with no solid has an empty list.
    pub validity: Vec<SolidValidity>,
    /// The model, retained for export.
    pub model: ModelFile,
}

impl ExecutedModel {
    /// Whether every solid is closed and valid — what "print-ready" means
    /// for the export gate.
    pub fn is_printable(&self) -> bool {
        !self.validity.is_empty() && self.validity.iter().all(|solid| solid.is_printable())
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
            model: ModelFile(PathBuf::from("/scratch/model-1.brep")),
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
            summary: ModelSummary {
                volume: 0.0,
                bounds_min: [0.0; 3],
                bounds_max: [0.0; 3],
                face_count: 0,
                edge_count: 0,
                vertex_count: 0,
            },
            validity,
            model: ModelFile(PathBuf::new()),
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
}
