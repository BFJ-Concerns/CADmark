// Geometry context types — the structured format sent to the AI
// when the user clicks an element in the viewport.

use serde::{Deserialize, Serialize};

use crate::ledger::LedgerValue;

/// A topological element the user can select in the viewport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TopologyElement {
    Part(PartId),
    Face(FaceId),
    Edge(EdgeId),
    Vertex(VertexId),
}

impl TopologyElement {
    /// Plain-language label such as "face 3" for people reading the UI.
    pub fn display_label(&self) -> String {
        match self {
            Self::Part(PartId(id)) => format!("part {id}"),
            Self::Face(FaceId(id)) => format!("face {id}"),
            Self::Edge(EdgeId(id)) => format!("edge {id}"),
            Self::Vertex(VertexId(id)) => format!("vertex {id}"),
        }
    }
}

/// Stable ordinal of a completed top-level part within one script execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartId(pub u32);

/// Unique identifier for a face in the rendered mesh.
/// Assigned during tessellation and used for GPU picking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FaceId(pub u32);

/// Unique identifier for an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeId(pub u32);

/// Unique identifier for a vertex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VertexId(pub u32);

/// Screen-space position for overlay placement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScreenPosition {
    pub x: f32,
    pub y: f32,
}

/// The current selection state in the viewport.
#[derive(Debug, Clone, Default)]
pub enum SelectionState {
    #[default]
    None,
    /// Element under the cursor — preview highlight, not yet clicked.
    Hovering(TopologyElement),
    /// Element the user has clicked — glow effect active.
    Selected(TopologyElement),
}

/// Geometry context packaged for the AI.
/// Stable output format regardless of which identification strategy produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeometryContext {
    /// The selected element type and ID.
    pub element: TopologyElement,
    /// Construction-time provenance: one source line, several candidate
    /// lines, or none. Never guessed; the UI and the AI both see which.
    pub provenance: LedgerValue,
    /// Experimental identification data — strategies can attach arbitrary
    /// key-value pairs here without changing the outer format.
    #[serde(default)]
    pub identification: std::collections::HashMap<String, String>,
    /// Lines from the executed script around the provenance candidates.  An
    /// untraced element carries the whole script, because there is no honest
    /// smaller window to choose.
    #[serde(default)]
    pub source_context: String,
    /// Elements directly incident to the selected element, measured from the
    /// final topology rather than inferred from positions.
    #[serde(default)]
    pub neighbours: Vec<TopologyElement>,
    /// The candidate the user picked when the provenance was ambiguous.
    ///
    /// Kept beside `provenance` rather than collapsing it: that the element
    /// was ambiguous and that a person resolved it are both facts the AI
    /// needs, and a resolved entry would carry neither.
    #[serde(default)]
    pub chosen_candidate: Option<crate::ledger::ProvenanceEntry>,
}

/// Measured geometry of one face in the final model, in model units (mm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceDescriptor {
    /// OCCT surface classification, e.g. "plane", "cylinder", "bspline".
    pub surface_type: String,
    pub area: f64,
    pub centre: [f64; 3],
    /// Outward-facing unit normal at the parametric centre.
    pub normal: [f64; 3],
    pub neighbours: Vec<TopologyElement>,
}

/// Measured geometry of one edge in the final model, in model units (mm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeDescriptor {
    /// OCCT curve classification, e.g. "line", "circle", "bspline".
    pub curve_type: String,
    pub length: f64,
    /// Circle radius when this is a circular edge; absent for other curves.
    pub radius: Option<f64>,
    pub centre: [f64; 3],
    pub neighbours: Vec<TopologyElement>,
}

/// Position of one vertex in the final model, in model units (mm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VertexDescriptor {
    pub position: [f64; 3],
    pub neighbours: Vec<TopologyElement>,
}

/// The closest separation between two selected topological elements, in mm.
/// This remains plain data because it crosses the kernel worker boundary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MinimumDistance {
    pub millimetres: f64,
}

impl MinimumDistance {
    /// A compact label suitable for the in-app measurement readout.
    pub fn describe(self) -> String {
        format!("Minimum distance {} mm", compact(self.millimetres))
    }
}

/// Whether one solid of the executed model is printable geometry: every
/// shell closed, and the kernel's own validity check passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolidValidity {
    /// Every shell of the solid is closed (no open boundary).
    pub closed: bool,
    /// OCCT's shape analyser found no defect.
    pub valid: bool,
}

impl SolidValidity {
    pub fn is_printable(self) -> bool {
        self.closed && self.valid
    }
}

/// Whole-model measurements used for status display and edit regression checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSummary {
    pub volume: f64,
    pub bounds_min: [f64; 3],
    pub bounds_max: [f64; 3],
    pub face_count: usize,
    pub edge_count: usize,
    pub vertex_count: usize,
}

impl ModelSummary {
    /// Extent along each axis.
    pub fn size(&self) -> [f64; 3] {
        std::array::from_fn(|axis| self.bounds_max[axis] - self.bounds_min[axis])
    }

    /// One-line description for the status bar.
    pub fn describe(&self) -> String {
        let size = self.size();
        format!(
            "{} faces, volume {} mm³, {} × {} × {} mm",
            self.face_count,
            compact(self.volume),
            compact(size[0]),
            compact(size[1]),
            compact(size[2])
        )
    }

    /// Describe what changed between a previous model and this one, so the
    /// user can spot an edit that did more than it should have. Returns None
    /// when nothing measurable changed.
    pub fn describe_change_from(&self, before: &ModelSummary) -> Option<String> {
        let mut parts = Vec::new();
        if before.face_count != self.face_count {
            parts.push(format!(
                "faces {} to {}",
                before.face_count, self.face_count
            ));
        }
        if !close(before.volume, self.volume) {
            let percent = if before.volume.abs() > f64::EPSILON {
                format!(
                    " ({:+.1}%)",
                    (self.volume - before.volume) / before.volume * 100.0
                )
            } else {
                String::new()
            };
            parts.push(format!(
                "volume {} to {} mm³{percent}",
                compact(before.volume),
                compact(self.volume)
            ));
        }
        let (before_size, after_size) = (before.size(), self.size());
        if (0..3).any(|axis| !close(before_size[axis], after_size[axis])) {
            parts.push(format!(
                "size {} × {} × {} to {} × {} × {} mm",
                compact(before_size[0]),
                compact(before_size[1]),
                compact(before_size[2]),
                compact(after_size[0]),
                compact(after_size[1]),
                compact(after_size[2])
            ));
        }
        if parts.is_empty() {
            return None;
        }
        let mut report = parts.join("; ");
        let large = before.volume.abs() > f64::EPSILON
            && ((self.volume - before.volume) / before.volume).abs() > 0.5;
        if large || self.volume.abs() < f64::EPSILON {
            report.push_str(". This is a large change; check the model is still what you intended");
        }
        Some(report)
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

/// Format a measurement with as few decimals as convey it.
pub fn compact(value: f64) -> String {
    if (value - value.round()).abs() < 5e-3 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.2}")
    }
}

/// Measured geometry for every element of the final model, indexed by the
/// same zero-based IDs the picking pass and provenance ledger use.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GeometryDescriptors {
    pub faces: Vec<FaceDescriptor>,
    pub edges: Vec<EdgeDescriptor>,
    pub vertices: Vec<VertexDescriptor>,
}

impl GeometryDescriptors {
    pub fn face(&self, id: FaceId) -> Option<&FaceDescriptor> {
        self.faces.get(id.0 as usize)
    }

    pub fn edge(&self, id: EdgeId) -> Option<&EdgeDescriptor> {
        self.edges.get(id.0 as usize)
    }

    pub fn vertex(&self, id: VertexId) -> Option<&VertexDescriptor> {
        self.vertices.get(id.0 as usize)
    }

    pub fn neighbours(&self, element: &TopologyElement) -> Vec<TopologyElement> {
        match element {
            // A part is the whole solid, not an element within the
            // descriptor tables, so it has no neighbours of its own.
            TopologyElement::Part(_) => None,
            TopologyElement::Face(id) => self
                .face(*id)
                .map(|descriptor| descriptor.neighbours.clone()),
            TopologyElement::Edge(id) => self
                .edge(*id)
                .map(|descriptor| descriptor.neighbours.clone()),
            TopologyElement::Vertex(id) => self
                .vertex(*id)
                .map(|descriptor| descriptor.neighbours.clone()),
        }
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(volume: f64, size: [f64; 3], faces: usize) -> ModelSummary {
        ModelSummary {
            volume,
            bounds_min: [0.0; 3],
            bounds_max: size,
            face_count: faces,
            edge_count: 0,
            vertex_count: 0,
        }
    }

    #[test]
    fn summary_describes_itself_compactly() {
        assert_eq!(
            summary(1000.0, [20.0, 10.0, 5.0], 6).describe(),
            "6 faces, volume 1000 mm³, 20 × 10 × 5 mm"
        );
        assert_eq!(
            summary(12.3456, [1.5, 1.0, 1.0], 6).describe(),
            "6 faces, volume 12.35 mm³, 1.50 × 1 × 1 mm"
        );
    }

    #[test]
    fn minimum_distance_describes_itself_compactly() {
        assert_eq!(
            MinimumDistance {
                millimetres: 12.3456
            }
            .describe(),
            "Minimum distance 12.35 mm"
        );
    }

    #[test]
    fn unchanged_model_reports_no_change() {
        let before = summary(1000.0, [20.0, 10.0, 5.0], 6);
        assert_eq!(before.describe_change_from(&before), None);
    }

    #[test]
    fn change_report_lists_faces_volume_and_size() {
        let before = summary(1000.0, [20.0, 10.0, 5.0], 6);
        let after = summary(1200.0, [20.0, 10.0, 8.0], 10);
        assert_eq!(
            after.describe_change_from(&before).unwrap(),
            "faces 6 to 10; volume 1000 to 1200 mm³ (+20.0%); size 20 × 10 × 5 to 20 × 10 × 8 mm"
        );
    }

    #[test]
    fn large_change_is_flagged() {
        let before = summary(1000.0, [20.0, 10.0, 5.0], 6);
        let after = summary(100.0, [20.0, 10.0, 5.0], 6);
        let report = after.describe_change_from(&before).unwrap();
        assert!(report.starts_with("volume 1000 to 100 mm³ (-90.0%)"));
        assert!(report.contains("large change"));
    }

    #[test]
    fn element_labels_are_plain() {
        assert_eq!(TopologyElement::Face(FaceId(3)).display_label(), "face 3");
        assert_eq!(TopologyElement::Part(PartId(1)).display_label(), "part 1");
        assert_eq!(TopologyElement::Edge(EdgeId(0)).display_label(), "edge 0");
    }
}
