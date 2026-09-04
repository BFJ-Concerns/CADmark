// Sketch lineage — which drawn curve a piece of the model came from.
//
// A sketch element carries the line that drew it. A solid element carries
// the sketch line only where the kernel's own per-operation history said so:
// an input the maker reported as generating or modifying that output passes
// its label on. Nothing else does. Where no maker answered, the element
// holds a stated absence naming what was asked, because a plausible wrong
// curve is worse than an admitted one.
//
// Transient like the provenance ledger: rebuilt on every execution, never
// persisted.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::geometry::{EdgeId, FaceId, SketchElement, VertexId};
use crate::ledger::{SemanticOperation, SourceRef};

/// The sketch object that drew an element, and the line it was drawn on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SketchSource {
    pub source: SourceRef,
    /// The stock sketch object, e.g. "Rectangle" or "Polyline".
    pub object: String,
}

impl SketchSource {
    /// Plain-language description such as "drawn by Rectangle at line 4".
    pub fn describe(&self) -> String {
        format!("drawn by {} at line {}", self.object, self.source.line)
    }
}

/// Why an element has no sketch route. Each case is something the kernel
/// was asked and answered, not an assumption about the geometry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoSketchRoute {
    /// Nothing in this element's construction was drawn in a sketch.
    NoSketchAncestor,
    /// The operation had sketch-drawn inputs and reported no history from
    /// any of them to this element.
    OperationHistoryEmpty {
        operation: SemanticOperation,
        source: SourceRef,
    },
    /// The element was merged by the clean-up step, which keeps no maker
    /// history, so the sketch it descends from cannot be named.
    CleanUpStep,
}

impl NoSketchRoute {
    pub fn describe(&self) -> String {
        match self {
            Self::NoSketchAncestor => {
                "no sketch route: nothing this element was built from was drawn in a sketch"
                    .to_string()
            }
            Self::OperationHistoryEmpty { operation, source } => format!(
                "no sketch route: the {} at line {} reports no sketch history for this element",
                operation.display_name(),
                source.line
            ),
            Self::CleanUpStep => {
                "no sketch route: the clean-up step merged this element and keeps no history \
                 through it"
                    .to_string()
            }
        }
    }
}

/// What is known about one element's sketch origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SketchLineage {
    Resolved(SketchSource),
    /// Several drawn curves reached this element and the kernel named them
    /// all; which one the user means is theirs to say.
    Ambiguous(Vec<SketchSource>),
    NoRoute(NoSketchRoute),
}

impl Default for SketchLineage {
    fn default() -> Self {
        Self::NoRoute(NoSketchRoute::NoSketchAncestor)
    }
}

impl SketchLineage {
    /// The single sketch source when exactly one drawn curve reached here.
    pub fn resolved(&self) -> Option<&SketchSource> {
        match self {
            Self::Resolved(source) => Some(source),
            Self::Ambiguous(_) | Self::NoRoute(_) => None,
        }
    }

    /// Every sketch source, in ledger order: none where there is no route.
    pub fn candidates(&self) -> &[SketchSource] {
        match self {
            Self::Resolved(source) => std::slice::from_ref(source),
            Self::Ambiguous(sources) => sources,
            Self::NoRoute(_) => &[],
        }
    }

    /// The stated absence, where there is one.
    pub fn no_route(&self) -> Option<&NoSketchRoute> {
        match self {
            Self::NoRoute(reason) => Some(reason),
            Self::Resolved(_) | Self::Ambiguous(_) => None,
        }
    }

    /// Plain-language description for the UI and the model.
    pub fn describe(&self) -> String {
        match self {
            Self::Resolved(source) => source.describe(),
            Self::Ambiguous(sources) => format!(
                "several sketch curves reach it: {}",
                sources
                    .iter()
                    .map(SketchSource::describe)
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
            Self::NoRoute(reason) => reason.describe(),
        }
    }
}

/// The sketch lineage of one execution: the sketch elements the user can
/// point at, and the sketch route of every element of the final solid.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SketchLineageEntries", into = "SketchLineageEntries")]
pub struct SketchLineageLedger {
    faces: HashMap<FaceId, SketchLineage>,
    edges: HashMap<EdgeId, SketchLineage>,
    vertices: HashMap<VertexId, SketchLineage>,
    elements: Vec<(SketchElement, SketchSource)>,
}

/// The ledger's wire shape — entry lists, because typed IDs do not
/// round-trip as JSON object keys.
#[derive(Serialize, Deserialize)]
struct SketchLineageEntries {
    faces: Vec<(FaceId, SketchLineage)>,
    edges: Vec<(EdgeId, SketchLineage)>,
    vertices: Vec<(VertexId, SketchLineage)>,
    elements: Vec<(SketchElement, SketchSource)>,
}

impl From<SketchLineageLedger> for SketchLineageEntries {
    fn from(ledger: SketchLineageLedger) -> Self {
        let mut faces: Vec<_> = ledger.faces.into_iter().collect();
        faces.sort_by_key(|(id, _)| id.0);
        let mut edges: Vec<_> = ledger.edges.into_iter().collect();
        edges.sort_by_key(|(id, _)| id.0);
        let mut vertices: Vec<_> = ledger.vertices.into_iter().collect();
        vertices.sort_by_key(|(id, _)| id.0);
        Self {
            faces,
            edges,
            vertices,
            elements: ledger.elements,
        }
    }
}

impl From<SketchLineageEntries> for SketchLineageLedger {
    fn from(entries: SketchLineageEntries) -> Self {
        Self {
            faces: entries.faces.into_iter().collect(),
            edges: entries.edges.into_iter().collect(),
            vertices: entries.vertices.into_iter().collect(),
            elements: entries.elements,
        }
    }
}

impl SketchLineageLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_face(&mut self, id: FaceId, lineage: SketchLineage) {
        self.faces.insert(id, lineage);
    }

    pub fn record_edge(&mut self, id: EdgeId, lineage: SketchLineage) {
        self.edges.insert(id, lineage);
    }

    pub fn record_vertex(&mut self, id: VertexId, lineage: SketchLineage) {
        self.vertices.insert(id, lineage);
    }

    /// Record a sketch element the user can point at. Elements are numbered
    /// per kind in the order the script drew them, which is the order the
    /// profile view draws them in.
    pub fn record_element(&mut self, element: SketchElement, source: SketchSource) {
        self.elements.push((element, source));
    }

    /// The sketch route of a solid element. An element the execution never
    /// recorded has no route rather than no answer: the user is told.
    pub fn lookup_face(&self, id: FaceId) -> SketchLineage {
        self.faces.get(&id).cloned().unwrap_or_default()
    }

    pub fn lookup_edge(&self, id: EdgeId) -> SketchLineage {
        self.edges.get(&id).cloned().unwrap_or_default()
    }

    pub fn lookup_vertex(&self, id: VertexId) -> SketchLineage {
        self.vertices.get(&id).cloned().unwrap_or_default()
    }

    /// The line that drew a sketch element the user clicked. A sketch
    /// element outside this execution resolves to no route, never to a
    /// neighbouring curve.
    pub fn lookup_element(&self, element: &SketchElement) -> SketchLineage {
        self.elements
            .iter()
            .find(|(candidate, _)| candidate == element)
            .map(|(_, source)| SketchLineage::Resolved(source.clone()))
            .unwrap_or_default()
    }

    pub fn elements(&self) -> &[(SketchElement, SketchSource)] {
        &self.elements
    }

    pub fn element_count(&self) -> usize {
        self.elements.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::SketchElementKind;

    fn source(line: u32, object: &str) -> SketchSource {
        SketchSource {
            source: SourceRef {
                line,
                code: "Rectangle(20, 10)".to_string(),
            },
            object: object.to_string(),
        }
    }

    #[test]
    fn an_unrecorded_solid_element_has_a_stated_absence_not_a_neighbour() {
        let mut ledger = SketchLineageLedger::new();
        ledger.record_face(FaceId(0), SketchLineage::Resolved(source(4, "Rectangle")));

        let unrecorded = ledger.lookup_face(FaceId(7));
        assert_eq!(unrecorded.resolved(), None);
        assert_eq!(
            unrecorded.no_route(),
            Some(&NoSketchRoute::NoSketchAncestor)
        );
    }

    #[test]
    fn an_unrecorded_sketch_element_resolves_to_no_route() {
        let mut ledger = SketchLineageLedger::new();
        ledger.record_element(
            SketchElement {
                kind: SketchElementKind::Curve,
                index: 0,
            },
            source(4, "Rectangle"),
        );

        let missing = ledger.lookup_element(&SketchElement {
            kind: SketchElementKind::Curve,
            index: 3,
        });
        assert!(missing.candidates().is_empty());
        assert_eq!(missing.no_route(), Some(&NoSketchRoute::NoSketchAncestor));
    }

    #[test]
    fn a_sketch_element_resolves_to_the_line_that_drew_it() {
        let mut ledger = SketchLineageLedger::new();
        ledger.record_element(
            SketchElement {
                kind: SketchElementKind::Curve,
                index: 2,
            },
            source(4, "Rectangle"),
        );
        ledger.record_element(
            SketchElement {
                kind: SketchElementKind::Corner,
                index: 2,
            },
            source(9, "Circle"),
        );

        let curve = ledger.lookup_element(&SketchElement {
            kind: SketchElementKind::Curve,
            index: 2,
        });
        assert_eq!(curve.resolved().map(|source| source.source.line), Some(4));
        let corner = ledger.lookup_element(&SketchElement {
            kind: SketchElementKind::Corner,
            index: 2,
        });
        assert_eq!(corner.resolved().map(|source| source.source.line), Some(9));
    }

    #[test]
    fn each_absence_says_what_was_asked() {
        assert_eq!(
            NoSketchRoute::OperationHistoryEmpty {
                operation: SemanticOperation::BooleanFuse,
                source: SourceRef {
                    line: 12,
                    code: "Box(5, 5, 5)".to_string(),
                },
            }
            .describe(),
            "no sketch route: the union at line 12 reports no sketch history for this element",
        );
        assert!(
            NoSketchRoute::CleanUpStep
                .describe()
                .contains("clean-up step")
        );
        assert!(
            NoSketchRoute::NoSketchAncestor
                .describe()
                .contains("drawn in a sketch")
        );
    }

    #[test]
    fn ambiguity_names_every_curve_that_reached_the_element() {
        let lineage = SketchLineage::Ambiguous(vec![source(4, "Rectangle"), source(9, "Circle")]);
        assert_eq!(lineage.candidates().len(), 2);
        assert_eq!(lineage.resolved(), None);
        assert_eq!(
            lineage.describe(),
            "several sketch curves reach it: drawn by Rectangle at line 4 or drawn by Circle at \
             line 9",
        );
    }

    #[test]
    fn the_ledger_round_trips_through_its_wire_shape() {
        let mut ledger = SketchLineageLedger::new();
        ledger.record_face(FaceId(1), SketchLineage::Resolved(source(4, "Rectangle")));
        ledger.record_edge(
            EdgeId(2),
            SketchLineage::NoRoute(NoSketchRoute::CleanUpStep),
        );
        ledger.record_vertex(VertexId(3), SketchLineage::default());
        ledger.record_element(
            SketchElement {
                kind: SketchElementKind::Region,
                index: 0,
            },
            source(4, "Rectangle"),
        );

        let json = serde_json::to_string(&ledger).unwrap();
        let restored: SketchLineageLedger = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, ledger);
    }
}
