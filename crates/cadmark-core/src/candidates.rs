// Reasoning over an ambiguous element's candidate source lines: the order
// they are offered in, and which geometry each one accounts for.
//
// Both answers come from the ledger and nothing else. The ledger records a
// relation (whether the operation claimed the element itself or an ancestor
// of it) and an operation identifier that counts construction order, and
// those two fields are the whole ranking material. Where they cannot
// separate the candidates the order is not a ranking, and `ranked` says so:
// an arbitrary order presented as a likelihood is the guess the whole
// provenance design refuses to make. Geometry is never consulted — identity
// here is born-labelled or it is unknown.

use crate::geometry::TopologyElement;
use crate::ledger::{ProvenanceEntry, ProvenanceLedger, ProvenanceRelation};

/// The order to offer candidates in, and whether the ledger determined it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateOrder {
    /// Indices into the candidate slice, most likely first when `ranked`.
    pub order: Vec<usize>,
    /// True when the ledger separated every candidate from every other, so
    /// the order carries meaning. False when it could not, in which case
    /// `order` is the ledger's own order and must be presented as unordered.
    pub ranked: bool,
}

/// Sort key from the ledger alone: a direct claim outranks an inherited
/// one, and among equals the later operation outranks the earlier.
fn rank_key(entry: &ProvenanceEntry) -> (u8, std::cmp::Reverse<u64>) {
    let directness = match entry.relation {
        ProvenanceRelation::Generated | ProvenanceRelation::Modified => 0,
        ProvenanceRelation::GeneratedDescendant | ProvenanceRelation::ModifiedDescendant => 1,
    };
    (directness, std::cmp::Reverse(entry.operation_id))
}

/// Order an element's candidate sources for presentation.
///
/// Ranks only when the ledger's own fields give every candidate a distinct
/// key. A single tie means the ledger cannot tell them apart, and the
/// result falls back to ledger order with `ranked` false rather than break
/// the tie on something the ledger did not record.
pub fn order_candidates(candidates: &[ProvenanceEntry]) -> CandidateOrder {
    let keys: Vec<_> = candidates.iter().map(rank_key).collect();
    let distinct = {
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        sorted.len() == keys.len()
    };
    if !distinct {
        return CandidateOrder {
            order: (0..candidates.len()).collect(),
            ranked: false,
        };
    }
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by_key(|&index| keys[index]);
    CandidateOrder {
        order,
        ranked: candidates.len() > 1,
    }
}

/// Every element the ledger attributes to one operation — the geometry that
/// candidate accounts for, and so what highlighting it should show.
///
/// An element counts when any of its recorded candidates names the
/// operation, so an ambiguous element belongs to each of its candidates'
/// footprints. Returned faces first, then edges, then vertices, each in ID
/// order, so the same ledger always yields the same footprint.
pub fn candidate_footprint(ledger: &ProvenanceLedger, operation_id: u64) -> Vec<TopologyElement> {
    let claims = |value: &crate::ledger::LedgerValue| {
        value
            .candidates()
            .iter()
            .any(|entry| entry.operation_id == operation_id)
    };
    let mut faces: Vec<_> = ledger
        .face_ids()
        .filter(|id| ledger.lookup_face(*id).is_some_and(claims))
        .collect();
    faces.sort_by_key(|id| id.0);
    let mut edges: Vec<_> = ledger
        .edge_ids()
        .filter(|id| ledger.lookup_edge(*id).is_some_and(claims))
        .collect();
    edges.sort_by_key(|id| id.0);
    let mut vertices: Vec<_> = ledger
        .vertex_ids()
        .filter(|id| ledger.lookup_vertex(*id).is_some_and(claims))
        .collect();
    vertices.sort_by_key(|id| id.0);
    faces
        .into_iter()
        .map(TopologyElement::Face)
        .chain(edges.into_iter().map(TopologyElement::Edge))
        .chain(vertices.into_iter().map(TopologyElement::Vertex))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{EdgeId, FaceId, VertexId};
    use crate::ledger::{LedgerValue, SemanticOperation, SourceRef};

    fn entry(line: u32, operation_id: u64, relation: ProvenanceRelation) -> ProvenanceEntry {
        ProvenanceEntry {
            source: SourceRef {
                line,
                code: format!("line {line}"),
            },
            operation: SemanticOperation::Fillet,
            operation_id,
            relation,
        }
    }

    #[test]
    fn the_later_direct_operation_is_offered_first() {
        let candidates = [
            entry(2, 1, ProvenanceRelation::Generated),
            entry(9, 4, ProvenanceRelation::Modified),
        ];
        let order = order_candidates(&candidates);
        assert!(order.ranked);
        assert_eq!(order.order, vec![1, 0]);
    }

    #[test]
    fn a_direct_claim_outranks_an_inherited_one_however_old() {
        let candidates = [
            entry(2, 9, ProvenanceRelation::ModifiedDescendant),
            entry(7, 1, ProvenanceRelation::Modified),
        ];
        let order = order_candidates(&candidates);
        assert!(order.ranked);
        assert_eq!(order.order, vec![1, 0]);
    }

    #[test]
    fn candidates_the_ledger_cannot_separate_keep_ledger_order_and_are_not_ranked() {
        let candidates = [
            entry(4, 3, ProvenanceRelation::Generated),
            entry(8, 3, ProvenanceRelation::Modified),
        ];
        let order = order_candidates(&candidates);
        assert!(
            !order.ranked,
            "two candidates sharing a construction step give the ledger nothing to rank on"
        );
        assert_eq!(order.order, vec![0, 1]);
    }

    #[test]
    fn a_footprint_holds_every_element_the_operation_claims_and_nothing_else() {
        let mut ledger = ProvenanceLedger::new();
        let fillet = entry(9, 4, ProvenanceRelation::Modified);
        let boxed = entry(2, 1, ProvenanceRelation::Generated);
        ledger
            .record_face(FaceId(1), LedgerValue::Resolved(boxed.clone()))
            .unwrap();
        ledger
            .record_face(FaceId(0), LedgerValue::Resolved(fillet.clone()))
            .unwrap();
        ledger
            .record_edge(
                EdgeId(5),
                LedgerValue::Ambiguous(vec![boxed.clone(), fillet.clone()]),
            )
            .unwrap();
        ledger
            .record_vertex(VertexId(2), LedgerValue::Untraced)
            .unwrap();
        // A traced vertex, so the footprint is proved to carry every kind
        // of element the ledger records and not just the two the viewport
        // can currently draw.
        ledger
            .record_vertex(VertexId(3), LedgerValue::Resolved(fillet.clone()))
            .unwrap();

        assert_eq!(
            candidate_footprint(&ledger, 4),
            vec![
                TopologyElement::Face(FaceId(0)),
                TopologyElement::Edge(EdgeId(5)),
                TopologyElement::Vertex(VertexId(3)),
            ]
        );
        assert_eq!(
            candidate_footprint(&ledger, 1),
            vec![
                TopologyElement::Face(FaceId(1)),
                TopologyElement::Edge(EdgeId(5)),
            ]
        );
        assert!(candidate_footprint(&ledger, 77).is_empty());
    }
}
