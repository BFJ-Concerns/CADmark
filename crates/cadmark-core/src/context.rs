// Geometry context resolution — bridges picking results to provenance data,
// producing the structured context sent to the AI.
//
// This module implements ADR-0003's three-layer architecture:
// 1. Provenance (foundation): which code generated the element.
// 2. Identification (experimental): which specific element was clicked.
// 3. Output format (stable): packages the result for the AI bridge.

use crate::geometry::{
    GeometryContext, GeometryDescriptors, PickedElement, SketchElement, TopologyElement,
};
use crate::ledger::{LedgerValue, ProvenanceLedger};
use crate::sketch::SketchProfile;
use crate::sketch_lineage::{SketchLineage, SketchLineageLedger};

/// The picked element is outside the ledger the current model was built
/// from: a stale pick after a reload, or a picking-buffer fault.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{} is not part of the current model", element.display_label())]
pub struct MissingElement {
    pub element: TopologyElement,
}

/// Strategy for identifying which specific element was clicked.
/// ADR-0003 mandates this layer is modular and experimental.
pub trait IdentificationStrategy: Send + Sync {
    /// Given a selected element, produce additional key-value identification
    /// data that helps the AI disambiguate. Returns an empty map if the
    /// strategy doesn't apply.
    fn identify(&self, element: &TopologyElement) -> std::collections::HashMap<String, String>;

    fn name(&self) -> &str;

    /// Directly incident final-topology elements for the selected element.
    fn neighbours(&self, _element: &TopologyElement) -> Vec<TopologyElement> {
        Vec::new()
    }
}

/// Null strategy — sends only provenance data, no additional identification.
/// This is the baseline: just the generating code line.
pub struct NullIdentification;

impl IdentificationStrategy for NullIdentification {
    fn identify(&self, _element: &TopologyElement) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }

    fn name(&self) -> &str {
        "null"
    }
}

/// Identification from measured geometry: surface or curve type, size,
/// position and orientation of the selected element, so the AI can reason
/// about which element was picked and where it sits.
pub struct MeasuredIdentification {
    pub descriptors: GeometryDescriptors,
}

fn triple(values: [f64; 3]) -> String {
    format!("({:.2}, {:.2}, {:.2})", values[0], values[1], values[2])
}

impl IdentificationStrategy for MeasuredIdentification {
    fn identify(&self, element: &TopologyElement) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        match element {
            TopologyElement::Part(_) => {}
            TopologyElement::Face(id) => {
                if let Some(face) = self.descriptors.face(*id) {
                    map.insert("surface".into(), face.surface_type.clone());
                    map.insert("area_mm2".into(), format!("{:.2}", face.area));
                    map.insert("centre_mm".into(), triple(face.centre));
                    map.insert("outward_normal".into(), triple(face.normal));
                }
            }
            TopologyElement::Edge(id) => {
                if let Some(edge) = self.descriptors.edge(*id) {
                    map.insert("curve".into(), edge.curve_type.clone());
                    map.insert("length_mm".into(), format!("{:.2}", edge.length));
                    map.insert("centre_mm".into(), triple(edge.centre));
                }
            }
            TopologyElement::Vertex(id) => {
                if let Some(vertex) = self.descriptors.vertex(*id) {
                    map.insert("position_mm".into(), triple(vertex.position));
                }
            }
        }
        map
    }

    fn name(&self) -> &str {
        "measured"
    }

    fn neighbours(&self, element: &TopologyElement) -> Vec<TopologyElement> {
        self.descriptors.neighbours(element)
    }
}

/// Resolve a picked element into a full geometry context for the AI.
///
/// Looks up the element in the provenance ledger to find the generating code,
/// then runs the identification strategy for additional disambiguation. An
/// ambiguous or untraced source is carried through as such rather than
/// refused: the user can still comment on the element, and both the overlay
/// and the AI are told exactly what is known about where it came from.
pub fn resolve_context(
    element: &TopologyElement,
    ledger: &ProvenanceLedger,
    strategy: &dyn IdentificationStrategy,
) -> Result<GeometryContext, MissingElement> {
    // Step 1: Provenance lookup.
    let provenance = match element {
        TopologyElement::Part(_) => None,
        TopologyElement::Face(id) => ledger.lookup_face(*id),
        TopologyElement::Edge(id) => ledger.lookup_edge(*id),
        TopologyElement::Vertex(id) => ledger.lookup_vertex(*id),
    };

    let provenance = provenance
        .ok_or_else(|| MissingElement {
            element: element.clone(),
        })?
        .clone();

    // Step 2: Identification strategy.
    let identification = strategy.identify(element);
    let neighbours = strategy.neighbours(element);

    // Step 3: Package into the stable output format.
    Ok(GeometryContext {
        part: None,
        element: PickedElement::Solid(element.clone()),
        provenance,
        identification,
        source_context: String::new(),
        neighbours,
        chosen_candidate: None,
        sketch: SketchLineage::default(),
    })
}

/// Attach the element's sketch route to an already resolved context.
///
/// The route is whatever the execution's sketch lineage holds for this
/// element — the drawn curve the kernel's history reached, or the stated
/// reason it reached none. A context resolved without a sketch lineage
/// keeps the same shape as one whose element has no sketch ancestor, so a
/// consumer never has to tell "not asked" from "no route" by guessing.
pub fn with_sketch_route(
    mut context: GeometryContext,
    lineage: &SketchLineageLedger,
) -> GeometryContext {
    context.sketch = match &context.element {
        PickedElement::Solid(TopologyElement::Part(_)) => SketchLineage::default(),
        PickedElement::Solid(TopologyElement::Face(id)) => lineage.lookup_face(*id),
        PickedElement::Solid(TopologyElement::Edge(id)) => lineage.lookup_edge(*id),
        PickedElement::Solid(TopologyElement::Vertex(id)) => lineage.lookup_vertex(*id),
        PickedElement::Sketch(element) => lineage.lookup_element(element),
    };
    context
}

/// Package a clicked sketch element as a context the rest of the
/// application anchors to exactly as it anchors to a solid element.
///
/// A sketch element has no place in the solid's provenance ledger — it is
/// what the script drew, not what the kernel built — so its provenance is
/// the sketch route itself, which names the line that drew it or the reason
/// no line can be named.
pub fn resolve_sketch_context(
    element: SketchElement,
    lineage: &SketchLineageLedger,
    profile: &SketchProfile,
) -> GeometryContext {
    let sketch = lineage.lookup_element(&element);
    GeometryContext {
        part: None,
        element: PickedElement::Sketch(element),
        // Nothing the kernel built claims a sketch element, so its ledger
        // provenance is honestly empty; the sketch route below carries the
        // line that drew it.
        provenance: LedgerValue::Untraced,
        identification: profile.identification(&element),
        source_context: String::new(),
        neighbours: Vec::new(),
        chosen_candidate: None,
        sketch,
    }
}

/// Attach the executed script context to an already resolved element. The
/// window is deliberately based only on known source candidates; where none
/// is known, the full script is the only honest context to provide.
pub fn with_source_context(mut context: GeometryContext, source: Option<&str>) -> GeometryContext {
    let Some(source) = source else {
        context.source_context = "The executed script text is unavailable.".to_string();
        return context;
    };
    let lines: Vec<_> = source.lines().collect();
    if lines.is_empty() {
        context.source_context = "The executed script is empty.".to_string();
        return context;
    }
    // A sketch element has no ledger provenance — nothing the kernel built
    // claims it — so the line the sketch route names is the only known
    // source there is, and the window is drawn around it.
    let mut candidates: Vec<u32> = context
        .provenance
        .candidates()
        .iter()
        .map(|entry| entry.source.line)
        .collect();
    candidates.extend(
        context
            .sketch
            .candidates()
            .iter()
            .map(|source| source.source.line),
    );
    candidates.sort_unstable();
    candidates.dedup();
    let windows = if candidates.is_empty() {
        vec![(1, lines.len())]
    } else {
        let mut windows = candidates
            .into_iter()
            .map(|line| {
                let line = line as usize;
                (line.saturating_sub(2).max(1), (line + 2).min(lines.len()))
            })
            .collect::<Vec<_>>();
        windows.sort_unstable();
        windows.dedup();
        windows
    };
    context.source_context = windows
        .into_iter()
        .map(|(first, last)| {
            let body = lines[first.saturating_sub(1)..last]
                .iter()
                .enumerate()
                .map(|(offset, line)| format!("{} | {line}", first + offset))
                .collect::<Vec<_>>()
                .join("\n");
            format!("lines {first}-{last}:\n{body}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    context
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{EdgeId, FaceId};
    use crate::ledger::{
        LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
    };

    #[test]
    fn resolve_face_with_provenance() {
        let mut ledger = ProvenanceLedger::new();
        ledger
            .record_face(
                FaceId(3),
                LedgerValue::Resolved(ProvenanceEntry {
                    source: SourceRef {
                        line: 5,
                        code: "box = Box(10, 10, 10)".to_string(),
                    },
                    operation: SemanticOperation::Box,
                    operation_id: 1,
                    relation: ProvenanceRelation::Generated,
                }),
            )
            .unwrap();

        let element = TopologyElement::Face(FaceId(3));
        let strategy = NullIdentification;
        let context = resolve_context(&element, &ledger, &strategy).unwrap();

        let entry = context.provenance.resolved().unwrap();
        assert_eq!(entry.source.line, 5);
        assert_eq!(entry.source.code, "box = Box(10, 10, 10)");
        assert_eq!(entry.describe(), "created by box at line 5");
        assert!(context.identification.is_empty());
    }

    #[test]
    fn resolve_outside_the_model_returns_error() {
        let ledger = ProvenanceLedger::new();
        let element = TopologyElement::Edge(EdgeId(99));
        let strategy = NullIdentification;
        let error = resolve_context(&element, &ledger, &strategy).unwrap_err();
        assert_eq!(error, MissingElement { element });
        assert_eq!(
            error.to_string(),
            "edge 99 is not part of the current model"
        );
    }

    #[test]
    fn resolve_untraced_element_carries_untraced_provenance() {
        let mut ledger = ProvenanceLedger::new();
        ledger
            .record_face(FaceId(2), LedgerValue::Untraced)
            .unwrap();
        let element = TopologyElement::Face(FaceId(2));
        let context = resolve_context(&element, &ledger, &NullIdentification).unwrap();
        assert_eq!(context.provenance, LedgerValue::Untraced);
        assert!(context.provenance.candidates().is_empty());
        assert!(context.provenance.describe().contains("no source line"));
        assert_eq!(ledger.untraced_count(), 1);
    }

    #[test]
    fn resolve_ambiguous_provenance_carries_all_candidates() {
        let first = ProvenanceEntry {
            source: SourceRef {
                line: 2,
                code: "Box(1, 1, 1)".into(),
            },
            operation: SemanticOperation::Box,
            operation_id: 1,
            relation: ProvenanceRelation::Generated,
        };
        let second = ProvenanceEntry {
            source: SourceRef {
                line: 3,
                code: "Cylinder(1, 1)".into(),
            },
            operation: SemanticOperation::Cylinder,
            operation_id: 2,
            relation: ProvenanceRelation::Generated,
        };
        let mut ledger = ProvenanceLedger::new();
        ledger
            .record_face(
                FaceId(0),
                LedgerValue::Ambiguous(vec![first.clone(), second.clone()]),
            )
            .unwrap();

        let element = TopologyElement::Face(FaceId(0));
        let context = resolve_context(&element, &ledger, &NullIdentification).unwrap();
        assert_eq!(
            context.provenance.candidates(),
            &[first.clone(), second.clone()]
        );
        assert_eq!(
            context.provenance.describe(),
            "source is ambiguous: created by box at line 2 or created by cylinder at line 3"
        );
    }

    #[test]
    fn measured_identification_describes_the_selected_face() {
        use crate::geometry::FaceDescriptor;

        let strategy = MeasuredIdentification {
            descriptors: GeometryDescriptors {
                faces: vec![FaceDescriptor {
                    surface_type: "plane".into(),
                    area: 200.0,
                    centre: [0.0, 0.0, 2.5],
                    normal: [0.0, 0.0, 1.0],
                    neighbours: Vec::new(),
                }],
                edges: Vec::new(),
                vertices: Vec::new(),
            },
        };
        let map = strategy.identify(&TopologyElement::Face(FaceId(0)));
        assert_eq!(map["surface"], "plane");
        assert_eq!(map["area_mm2"], "200.00");
        assert_eq!(map["centre_mm"], "(0.00, 0.00, 2.50)");
        assert_eq!(map["outward_normal"], "(0.00, 0.00, 1.00)");
        assert!(
            strategy
                .identify(&TopologyElement::Face(FaceId(7)))
                .is_empty()
        );
    }

    #[test]
    fn custom_strategy_adds_identification() {
        struct TestStrategy;
        impl IdentificationStrategy for TestStrategy {
            fn identify(
                &self,
                element: &TopologyElement,
            ) -> std::collections::HashMap<String, String> {
                let mut map = std::collections::HashMap::new();
                if let TopologyElement::Face(id) = element {
                    map.insert("face_index".to_string(), id.0.to_string());
                    map.insert("method".to_string(), "test".to_string());
                }
                map
            }

            fn name(&self) -> &str {
                "test"
            }
        }

        let mut ledger = ProvenanceLedger::new();
        let element = TopologyElement::Face(FaceId(7));
        ledger
            .record_face(
                FaceId(7),
                LedgerValue::Resolved(ProvenanceEntry {
                    source: SourceRef {
                        line: 1,
                        code: "Box(1, 1, 1)".into(),
                    },
                    operation: SemanticOperation::Box,
                    operation_id: 1,
                    relation: ProvenanceRelation::Generated,
                }),
            )
            .unwrap();
        let strategy = TestStrategy;
        let context = resolve_context(&element, &ledger, &strategy).unwrap();

        assert_eq!(context.identification.get("face_index").unwrap(), "7");
        assert_eq!(context.identification.get("method").unwrap(), "test");
    }

    #[test]
    fn context_carries_measured_neighbours_and_honest_source_context() {
        use crate::geometry::{EdgeDescriptor, FaceDescriptor, VertexDescriptor};

        let mut ledger = ProvenanceLedger::new();
        ledger
            .record_face(FaceId(0), LedgerValue::Untraced)
            .unwrap();
        let strategy = MeasuredIdentification {
            descriptors: GeometryDescriptors {
                faces: vec![FaceDescriptor {
                    surface_type: "plane".into(),
                    area: 1.0,
                    centre: [0.0; 3],
                    normal: [0.0, 0.0, 1.0],
                    neighbours: vec![TopologyElement::Edge(EdgeId(2))],
                }],
                edges: vec![EdgeDescriptor {
                    curve_type: "line".into(),
                    length: 1.0,
                    radius: None,
                    centre: [0.0; 3],
                    neighbours: vec![TopologyElement::Face(FaceId(0))],
                }],
                vertices: vec![VertexDescriptor {
                    position: [0.0; 3],
                    neighbours: Vec::new(),
                }],
            },
        };
        let context =
            resolve_context(&TopologyElement::Face(FaceId(0)), &ledger, &strategy).unwrap();
        let context = with_source_context(context, Some("with BuildPart():\n    Box(1, 1, 1)"));

        assert_eq!(context.neighbours, vec![TopologyElement::Edge(EdgeId(2))]);
        assert!(context.source_context.contains("1 | with BuildPart():"));
        assert!(context.source_context.contains("2 |     Box(1, 1, 1)"));
    }

    #[test]
    fn a_resolved_context_takes_the_sketch_route_of_its_own_element() {
        let mut lineage = SketchLineageLedger::new();
        lineage.record_face(
            FaceId(1),
            SketchLineage::Resolved(crate::sketch_lineage::SketchSource {
                source: SourceRef {
                    line: 5,
                    code: "Rectangle(20, 10)".to_string(),
                },
                object: "Rectangle".to_string(),
            }),
        );

        let context = |element| GeometryContext {
            part: None,
            sketch: SketchLineage::default(),
            element,
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
        };

        let drawn = with_sketch_route(
            context(PickedElement::Solid(TopologyElement::Face(FaceId(1)))),
            &lineage,
        );
        assert_eq!(
            drawn.sketch.resolved().map(|source| source.source.line),
            Some(5),
        );
        // A different element must not inherit the only route on record.
        let other = with_sketch_route(
            context(PickedElement::Solid(TopologyElement::Face(FaceId(2)))),
            &lineage,
        );
        assert_eq!(other.sketch.resolved(), None);
        assert!(other.sketch.no_route().is_some());
    }

    #[test]
    fn a_clicked_sketch_element_becomes_an_anchorable_context() {
        use crate::geometry::{SketchElement, SketchElementKind};
        let mut lineage = SketchLineageLedger::new();
        let curve = SketchElement {
            kind: SketchElementKind::Curve,
            index: 2,
        };
        lineage.record_element(
            curve,
            SketchLineage::Resolved(crate::sketch_lineage::SketchSource {
                source: SourceRef {
                    line: 8,
                    code: "Rectangle(20, 10)".to_string(),
                },
                object: "Rectangle".to_string(),
            }),
        );
        let profile = SketchProfile {
            curves: vec![crate::sketch::SketchCurve {
                curve_id: 2,
                points: vec![[0.0; 3], [20.0, 0.0, 0.0]],
                curve_type: "line".to_string(),
                length: 20.0,
                radius: None,
            }],
            ..SketchProfile::default()
        };

        let context = with_source_context(
            resolve_sketch_context(curve, &lineage, &profile),
            Some(
                "from build123d import *\n\n\n\n\n\nwith BuildSketch():\n    Rectangle(20, 10)\nextrude(amount=5)\n",
            ),
        );

        // The context anchors to the sketch element itself, so it can be
        // held as a comment anchor exactly as a solid element is.
        assert_eq!(context.element, PickedElement::Sketch(curve));
        assert_eq!(
            context.sketch.resolved().map(|source| source.source.line),
            Some(8),
        );
        // Nothing the kernel built claims a drawn curve, so the window
        // comes from the sketch route rather than from ledger provenance.
        assert_eq!(context.provenance, LedgerValue::Untraced);
        // The curve's own measurements ride with the anchor, as a solid
        // edge's do.
        assert_eq!(
            context.identification.get("length_mm").map(String::as_str),
            Some("20.00")
        );
        assert!(context.source_context.contains("8 |     Rectangle(20, 10)"));
        assert!(!context.source_context.contains("1 | from build123d"));

        // An element the execution never recorded says so rather than
        // borrowing the only route on file.
        let unknown = resolve_sketch_context(
            SketchElement {
                kind: SketchElementKind::Corner,
                index: 9,
            },
            &lineage,
            &profile,
        );
        assert!(unknown.sketch.no_route().is_some());
    }
}
