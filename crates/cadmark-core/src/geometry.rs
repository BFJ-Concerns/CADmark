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

/// What a sketch object drew, as the user points at it: the curves
/// themselves, the corners where they meet, and the regions they enclose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SketchElementKind {
    Curve,
    Corner,
    Region,
}

impl SketchElementKind {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Curve => "curve",
            Self::Corner => "corner",
            Self::Region => "region",
        }
    }
}

/// An element of a sketch the user can select. Numbered per kind in the
/// order the script drew them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SketchElement {
    pub kind: SketchElementKind,
    pub index: u32,
}

impl SketchElement {
    /// Plain-language label such as "sketch curve 3".
    pub fn display_label(&self) -> String {
        format!("sketch {} {}", self.kind.display_name(), self.index)
    }
}

/// What a click landed on: a piece of the solid, or a piece of a sketch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PickedElement {
    Solid(TopologyElement),
    Sketch(SketchElement),
}

impl PickedElement {
    pub fn display_label(&self) -> String {
        match self {
            Self::Solid(element) => element.display_label(),
            Self::Sketch(element) => element.display_label(),
        }
    }

    /// The solid topology this pick names, where it names solid topology.
    pub fn solid(&self) -> Option<&TopologyElement> {
        match self {
            Self::Solid(element) => Some(element),
            Self::Sketch(_) => None,
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
    Hovering(PickedElement),
    /// Element the user has clicked — glow effect active.
    Selected(PickedElement),
}

/// Geometry context packaged for the AI.
/// Stable output format regardless of which identification strategy produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeometryContext {
    /// What the user selected: a piece of the solid, or a sketch element.
    pub element: PickedElement,
    /// The part whose local numbering `element` uses, where the model has
    /// parts: face, edge and vertex IDs restart in every part, so the
    /// element alone does not say which solid it is on. Filled by the
    /// application, which knows which part's ledger resolved the element;
    /// a sketch element belongs to no part.
    #[serde(default)]
    pub part: Option<PartId>,
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
    /// The sketch curve this element descends from, where the kernel's own
    /// history reached one, or the stated reason there is no route to a
    /// sketch at all.
    #[serde(default)]
    pub sketch: crate::sketch_lineage::SketchLineage,
}

impl GeometryContext {
    /// A context for `element` with the provenance the ledger gave it and
    /// nothing else filled in: no part, no identification, no source
    /// window, no neighbours, no chosen candidate, no sketch route. The
    /// application sets what it knows beyond that afterwards, and a test
    /// builds on this naming only the fields it reads, so a field added to
    /// the type is filled here and nowhere else.
    pub fn new(element: PickedElement, provenance: LedgerValue) -> Self {
        Self {
            element,
            part: None,
            provenance,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
            sketch: Default::default(),
        }
    }

    /// The solid element this context anchors to, where it anchors to one.
    /// A sketch anchor has none: sketch elements are not solid topology,
    /// so measurement and neighbour queries do not apply to them.
    pub fn solid(&self) -> Option<&TopologyElement> {
        match &self.element {
            PickedElement::Solid(element) => Some(element),
            PickedElement::Sketch(_) => None,
        }
    }
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
}

/// One part's measurements, named by the script binding that produced it,
/// so a report can say which part changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartMeasurements {
    pub name: String,
    pub summary: ModelSummary,
}

impl PartMeasurements {
    pub fn new(name: impl Into<String>, summary: ModelSummary) -> Self {
        Self {
            name: name.into(),
            summary,
        }
    }
}

/// Every part's measurements on one line, `name: measurements` separated by
/// semicolons, in the order given.
pub fn describe_parts(parts: &[PartMeasurements]) -> String {
    parts
        .iter()
        .map(|part| format!("{}: {}", part.name, part.summary.describe()))
        .collect::<Vec<_>>()
        .join("; ")
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
            bounds_max: size,
            face_count: faces,
            ..Default::default()
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
    fn parts_describe_themselves_by_name_on_one_line() {
        let parts = [
            PartMeasurements::new("bracket", summary(1000.0, [20.0, 10.0, 5.0], 6)),
            PartMeasurements::new("lid", summary(200.0, [20.0, 10.0, 1.0], 6)),
        ];
        assert_eq!(
            describe_parts(&parts),
            "bracket: 6 faces, volume 1000 mm³, 20 × 10 × 5 mm; lid: 6 faces, volume 200 mm³, 20 × 10 × 1 mm"
        );
    }

    #[test]
    fn element_labels_are_plain() {
        assert_eq!(TopologyElement::Face(FaceId(3)).display_label(), "face 3");
        assert_eq!(TopologyElement::Part(PartId(1)).display_label(), "part 1");
        assert_eq!(TopologyElement::Edge(EdgeId(0)).display_label(), "edge 0");
    }
}
