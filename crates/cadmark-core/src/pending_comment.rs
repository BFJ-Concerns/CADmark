// Pending spatial comments — the editable, unsent part of a turn.
//
// A card gets its display number when it is created.  That number is never
// derived from its position in the collection: removing one card therefore
// cannot silently change the marker-to-card pairing of the cards that remain.

use serde::{Deserialize, Serialize};

use crate::geometry::{GeometryContext, TopologyElement};

/// Stable identity for one pending card and its viewport marker cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PendingCommentId(pub u64);

/// One anchor shown on a pending comment card.
///
/// Reconciliation can replace a live anchor with `Lost` after a rebuild
/// without changing the card identity or the rest of its anchors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PendingAnchor {
    Live(GeometryContext),
    Lost { element: TopologyElement },
}

impl PendingAnchor {
    pub fn element(&self) -> &TopologyElement {
        match self {
            Self::Live(context) => &context.element,
            Self::Lost { element } => element,
        }
    }

    pub fn live_context(&self) -> Option<&GeometryContext> {
        match self {
            Self::Live(context) => Some(context),
            Self::Lost { .. } => None,
        }
    }
}

/// A user-editable spatial comment that has not yet been sent to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingComment {
    pub id: PendingCommentId,
    /// Human-visible marker number. Stable for this card's lifetime.
    pub marker_number: u64,
    pub text: String,
    pub anchors: Vec<PendingAnchor>,
}

impl PendingComment {
    /// The stable colour shared by this card and its viewport markers.
    pub fn marker_colour(&self) -> [f32; 4] {
        // The golden-ratio step spreads successive cards around the hue
        // wheel without a short repeating palette that would make two live
        // cards indistinguishable.
        let hue = (self.marker_number as f32 * 0.618_034).fract();
        let sector = hue * 6.0;
        let chroma = 0.72;
        let secondary = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
        let (red, green, blue) = match sector as u32 {
            0 => (chroma, secondary, 0.0),
            1 => (secondary, chroma, 0.0),
            2 => (0.0, chroma, secondary),
            3 => (0.0, secondary, chroma),
            4 => (secondary, 0.0, chroma),
            _ => (chroma, 0.0, secondary),
        };
        let lightness = 0.20;
        [red + lightness, green + lightness, blue + lightness, 0.78]
    }

    pub fn is_sendable(&self) -> bool {
        !self.text.trim().is_empty()
            && !self.anchors.is_empty()
            && self
                .anchors
                .iter()
                .all(|anchor| anchor.live_context().is_some())
    }

    pub fn live_anchors(&self) -> Option<Vec<GeometryContext>> {
        self.anchors
            .iter()
            .map(|anchor| anchor.live_context().cloned())
            .collect()
    }
}

/// The pending cards for one open design state.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingComments {
    next_id: u64,
    comments: Vec<PendingComment>,
}

impl PendingComments {
    pub fn add(&mut self, text: String, anchors: Vec<GeometryContext>) -> PendingCommentId {
        let id = PendingCommentId(self.next_id);
        self.next_id += 1;
        self.comments.push(PendingComment {
            id,
            marker_number: id.0 + 1,
            text,
            anchors: anchors.into_iter().map(PendingAnchor::Live).collect(),
        });
        id
    }

    pub fn remove(&mut self, id: PendingCommentId) -> Option<PendingComment> {
        let index = self.comments.iter().position(|comment| comment.id == id)?;
        Some(self.comments.remove(index))
    }

    pub fn comments(&self) -> &[PendingComment] {
        &self.comments
    }

    pub fn comments_mut(&mut self) -> &mut [PendingComment] {
        &mut self.comments
    }

    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }

    pub fn can_send(&self) -> bool {
        !self.comments.is_empty() && self.comments.iter().all(PendingComment::is_sendable)
    }

    pub fn drain(&mut self) -> Vec<PendingComment> {
        std::mem::take(&mut self.comments)
    }

    /// Put back a batch whose turn could not be started, retaining its IDs
    /// and marker pairing rather than creating replacement cards.
    pub fn restore(&mut self, comments: Vec<PendingComment>) {
        debug_assert!(self.comments.is_empty());
        self.comments = comments;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{FaceId, GeometryContext};
    use crate::ledger::LedgerValue;

    fn anchor(face: u32) -> GeometryContext {
        GeometryContext {
            element: TopologyElement::Face(FaceId(face)),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
        }
    }

    #[test]
    fn removing_a_card_keeps_each_remaining_marker_pairing_stable() {
        let mut pending = PendingComments::default();
        let first = pending.add("round this".into(), vec![anchor(1)]);
        let second = pending.add("chamfer this".into(), vec![anchor(2)]);
        let third = pending.add("make this taller".into(), vec![anchor(3)]);

        assert_eq!(pending.remove(second).unwrap().marker_number, 2);
        assert_eq!(
            pending
                .comments()
                .iter()
                .map(|comment| (comment.id, comment.marker_number, comment.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(first, 1, "round this"), (third, 3, "make this taller")]
        );
    }

    #[test]
    fn editing_a_card_through_the_live_collection_keeps_other_cards_intact() {
        let mut pending = PendingComments::default();
        pending.add("round this".into(), vec![anchor(1)]);
        pending.add("chamfer this".into(), vec![anchor(2)]);

        pending.comments_mut()[0].text = "round this more".into();

        assert_eq!(pending.comments()[0].text, "round this more");
        assert_eq!(pending.comments()[1].text, "chamfer this");
        assert_eq!(pending.comments()[1].marker_number, 2);
    }

    #[test]
    fn a_lost_anchor_keeps_its_card_but_prevents_submission() {
        let mut pending = PendingComments::default();
        let id = pending.add("round this".into(), vec![anchor(1)]);
        pending.comments[0].anchors[0] = PendingAnchor::Lost {
            element: TopologyElement::Face(FaceId(1)),
        };

        assert_eq!(pending.comments()[0].id, id);
        assert!(!pending.can_send());
        assert_eq!(pending.comments()[0].live_anchors(), None);
    }

    #[test]
    fn restoring_an_unsent_batch_keeps_its_original_pairing() {
        let mut pending = PendingComments::default();
        pending.add("round this".into(), vec![anchor(1)]);
        let second = pending.add("chamfer this".into(), vec![anchor(2)]);
        let batch = pending.drain();

        pending.restore(batch);

        assert_eq!(pending.comments()[1].id, second);
        assert_eq!(pending.comments()[1].marker_number, 2);
    }
}
