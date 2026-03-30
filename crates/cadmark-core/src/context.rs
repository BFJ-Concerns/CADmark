// Geometry context resolution — bridges picking results to provenance data,
// producing the structured context sent to the AI.
//
// This module implements ADR-0003's three-layer architecture:
// 1. Provenance (foundation): which code generated the element.
// 2. Identification (experimental): which specific element was clicked.
// 3. Output format (stable): packages the result for the AI bridge.

use crate::geometry::{GeometryContext, TopologyElement};
use crate::ledger::ProvenanceLedger;

/// Strategy for identifying which specific element was clicked.
/// ADR-0003 mandates this layer is modular and experimental.
pub trait IdentificationStrategy: Send + Sync {
    /// Given a selected element, produce additional key-value identification
    /// data that helps the AI disambiguate. Returns an empty map if the
    /// strategy doesn't apply.
    fn identify(
        &self,
        element: &TopologyElement,
    ) -> std::collections::HashMap<String, String>;

    fn name(&self) -> &str;
}

/// Null strategy — sends only provenance data, no additional identification.
/// This is the baseline: just the generating code line.
pub struct NullIdentification;

impl IdentificationStrategy for NullIdentification {
    fn identify(
        &self,
        _element: &TopologyElement,
    ) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }

    fn name(&self) -> &str {
        "null"
    }
}

/// Resolve a picked element into a full geometry context for the AI.
///
/// Looks up the element in the provenance ledger to find the generating code,
/// then runs the identification strategy for additional disambiguation.
pub fn resolve_context(
    element: &TopologyElement,
    ledger: &ProvenanceLedger,
    strategy: &dyn IdentificationStrategy,
) -> GeometryContext {
    // Step 1: Provenance lookup.
    let provenance = match element {
        TopologyElement::Face(id) => ledger.lookup_face(*id),
        TopologyElement::Edge(id) => ledger.lookup_edge(*id),
        TopologyElement::Vertex(id) => ledger.lookup_vertex(*id),
    };

    let source_line = provenance.map(|e| e.source.line);
    let source_code = provenance.map(|e| e.source.code.clone());

    // Step 2: Identification strategy.
    let identification = strategy.identify(element);

    // Step 3: Package into the stable output format.
    GeometryContext {
        element: element.clone(),
        source_line,
        source_code,
        identification,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{EdgeId, FaceId};
    use crate::ledger::{ProvenanceEntry, ProvenanceKind, SourceRef};

    #[test]
    fn resolve_face_with_provenance() {
        let mut ledger = ProvenanceLedger::new();
        ledger.record_face(
            FaceId(3),
            ProvenanceEntry {
                source: SourceRef {
                    line: 5,
                    code: "box = Box(10, 10, 10)".to_string(),
                },
                kind: ProvenanceKind::Generated,
            },
        );

        let element = TopologyElement::Face(FaceId(3));
        let strategy = NullIdentification;
        let context = resolve_context(&element, &ledger, &strategy);

        assert_eq!(context.source_line, Some(5));
        assert_eq!(
            context.source_code.as_deref(),
            Some("box = Box(10, 10, 10)")
        );
        assert!(context.identification.is_empty());
    }

    #[test]
    fn resolve_without_provenance_returns_none_fields() {
        let ledger = ProvenanceLedger::new();
        let element = TopologyElement::Edge(EdgeId(99));
        let strategy = NullIdentification;
        let context = resolve_context(&element, &ledger, &strategy);

        assert_eq!(context.source_line, None);
        assert_eq!(context.source_code, None);
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
                match element {
                    TopologyElement::Face(id) => {
                        map.insert("face_index".to_string(), id.0.to_string());
                        map.insert("method".to_string(), "test".to_string());
                    }
                    _ => {}
                }
                map
            }

            fn name(&self) -> &str {
                "test"
            }
        }

        let ledger = ProvenanceLedger::new();
        let element = TopologyElement::Face(FaceId(7));
        let strategy = TestStrategy;
        let context = resolve_context(&element, &ledger, &strategy);

        assert_eq!(context.identification.get("face_index").unwrap(), "7");
        assert_eq!(context.identification.get("method").unwrap(), "test");
    }
}
