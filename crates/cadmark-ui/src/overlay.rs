// Comment overlay — appears near selected geometry with a connecting line.
//
// Positioned in screen space near the first selection point and kept
// inside the viewport. The user writes free text and submits with Enter;
// Escape cancels. Clicking more geometry while the overlay is open adds
// anchors, so one comment can point at several elements.

use cadmark_core::candidates::order_candidates;
use cadmark_core::geometry::{GeometryContext, ScreenPosition};
use cadmark_core::ledger::{LedgerValue, ProvenanceEntry};

use crate::theme;

fn context_summary(context: &GeometryContext) -> String {
    let mut summary = format!(
        "{}: {}",
        context.element.display_label(),
        context.provenance.describe()
    );
    if let Some(surface) = context
        .identification
        .get("surface")
        .or_else(|| context.identification.get("curve"))
    {
        summary.push_str(&format!(" ({surface})"));
    }
    summary
}

/// Offer every candidate source line of each ambiguous anchor, and record
/// which one the pointer is over and which one the user chose.
///
/// Ordering comes from the ledger or not at all: where it cannot rank the
/// candidates the list says so and stays in the order the ledger recorded,
/// so an arbitrary order is never read as a likelihood.
fn show_candidate_choices(
    ui: &mut egui::Ui,
    anchors: &mut [GeometryContext],
    hovered_candidate: &mut Option<ProvenanceEntry>,
) {
    *hovered_candidate = None;
    for context in anchors.iter_mut() {
        let LedgerValue::Ambiguous(candidates) = context.provenance.clone() else {
            continue;
        };
        let ordering = order_candidates(&candidates);
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{} could have come from {} lines. {}",
                    context.element.display_label(),
                    candidates.len(),
                    if ordering.ranked {
                        "Most likely first."
                    } else {
                        "CADmark cannot tell which is likeliest; these are in the order it \
                         recorded them."
                    }
                ))
                .small()
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
        for &index in &ordering.order {
            let candidate = &candidates[index];
            let is_chosen = context.chosen_candidate.as_ref() == Some(candidate);
            let row = ui.selectable_label(
                is_chosen,
                egui::RichText::new(format!(
                    "{}: `{}`",
                    candidate.describe(),
                    candidate.source.code
                ))
                .small(),
            );
            if row.hovered() {
                *hovered_candidate = Some(candidate.clone());
            }
            if row.clicked() {
                context.chosen_candidate = if is_chosen {
                    None
                } else {
                    Some(candidate.clone())
                };
            }
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(match &context.chosen_candidate {
                    Some(chosen) => format!("Sending line {} alone.", chosen.source.line),
                    None => format!(
                        "No line chosen: sending all {} for the AI to decide.",
                        candidates.len()
                    ),
                })
                .small()
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
    }
}

/// State for the spatial comment overlay.
#[derive(Debug, Default)]
pub enum OverlayState {
    /// No overlay visible.
    #[default]
    Hidden,
    /// Overlay is open at a screen position, user is writing.
    Active {
        /// Where the first selected element is on screen.
        anchor: ScreenPosition,
        /// The text the user is writing.
        text: String,
        /// Every anchored element, in the order they were clicked, with
        /// the provenance resolved at pick time.
        anchors: Vec<GeometryContext>,
        /// Whether the text field has been given focus since opening.
        focused: bool,
        /// The candidate row the pointer is over, recomputed each frame so
        /// the viewport and code panel can show what that line accounts for.
        hovered_candidate: Option<ProvenanceEntry>,
    },
}

/// Result of showing the overlay — what action the user took.
pub enum OverlayAction {
    /// No action — overlay still open or hidden.
    None,
    /// User submitted a spatial comment anchored to these elements.
    Submit {
        text: String,
        anchors: Vec<GeometryContext>,
    },
    /// User cancelled (Escape).
    Cancel,
}

const OVERLAY_WIDTH: f32 = 300.0;

impl OverlayState {
    /// Open the overlay at the given screen position with one anchor.
    pub fn open(&mut self, anchor: ScreenPosition, context: GeometryContext) {
        *self = Self::Active {
            anchor,
            text: String::new(),
            anchors: vec![context],
            focused: false,
            hovered_candidate: None,
        };
    }

    /// Add an anchor to the open comment; clicking an element already
    /// anchored removes it. Returns whether the overlay was open.
    pub fn toggle_anchor(&mut self, context: GeometryContext) -> bool {
        let Self::Active { anchors, .. } = self else {
            return false;
        };
        match anchors
            .iter()
            .position(|anchor| anchor.element == context.element)
        {
            Some(index) if anchors.len() > 1 => {
                anchors.remove(index);
            }
            Some(_) => {}
            None => anchors.push(context),
        }
        true
    }

    /// The elements the open comment is anchored to.
    pub fn anchors(&self) -> &[GeometryContext] {
        match self {
            Self::Active { anchors, .. } => anchors,
            Self::Hidden => &[],
        }
    }

    /// The candidate line the pointer is resting on, if any. Hovering a
    /// candidate is what shows the user which geometry that line accounts
    /// for; the caller drives the highlights from this.
    pub fn hovered_candidate(&self) -> Option<&ProvenanceEntry> {
        match self {
            Self::Active {
                hovered_candidate, ..
            } => hovered_candidate.as_ref(),
            Self::Hidden => None,
        }
    }

    /// Close the overlay.
    pub fn close(&mut self) {
        *self = Self::Hidden;
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    /// Render the overlay within `bounds` (the viewport rectangle). Returns
    /// the action the user took.
    pub fn show(&mut self, ui: &mut egui::Ui, bounds: egui::Rect) -> OverlayAction {
        match self {
            Self::Hidden => OverlayAction::None,
            Self::Active {
                anchor,
                text,
                anchors,
                focused,
                hovered_candidate,
            } => {
                let mut action = OverlayAction::None;
                let anchor_pos = egui::pos2(anchor.x, anchor.y);

                // Prefer the right of the anchor; flip left near the edge.
                let estimated_height = 150.0;
                let mut overlay_pos = egui::pos2(anchor.x + 24.0, anchor.y - 12.0);
                if overlay_pos.x + OVERLAY_WIDTH > bounds.right() - 8.0 {
                    overlay_pos.x = anchor.x - 24.0 - OVERLAY_WIDTH;
                }
                overlay_pos.x = overlay_pos.x.max(bounds.left() + 8.0);
                overlay_pos.y = overlay_pos
                    .y
                    .min(bounds.bottom() - estimated_height - 8.0)
                    .max(bounds.top() + 8.0);

                let area = egui::Area::new(egui::Id::new("spatial_comment_overlay"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(overlay_pos)
                    .constrain_to(bounds)
                    .show(ui.ctx(), |ui| {
                        theme::tinted_card(theme::SPATIAL)
                            .shadow(ui.style().visuals.popup_shadow)
                            .show(ui, |ui| {
                                ui.set_width(OVERLAY_WIDTH);
                                ui.horizontal_wrapped(|ui| {
                                    for context in anchors.iter() {
                                        theme::chip(
                                            ui,
                                            &context.element.display_label(),
                                            theme::SPATIAL,
                                        )
                                        .on_hover_text(context_summary(context));
                                    }
                                    ui.label(
                                        egui::RichText::new(if anchors.len() == 1 {
                                            "Comment on this"
                                        } else {
                                            "Comment on these"
                                        })
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        egui::RichText::new("\u{00D7}")
                                                            .color(theme::TEXT_MUTED),
                                                    )
                                                    .frame(false),
                                                )
                                                .on_hover_text("Cancel (Esc)")
                                                .clicked()
                                            {
                                                action = OverlayAction::Cancel;
                                            }
                                        },
                                    );
                                });
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(match anchors.as_slice() {
                                            [only] => context_summary(only),
                                            _ => "Click more geometry to add anchors; click an \
                                                  anchored element again to remove it."
                                                .to_string(),
                                        })
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                    )
                                    .wrap(),
                                );

                                show_candidate_choices(ui, anchors, hovered_candidate);

                                let response = egui::Frame::new()
                                    .fill(theme::SUNKEN)
                                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                                    .corner_radius(egui::CornerRadius::same(theme::RADIUS))
                                    .inner_margin(egui::Margin::symmetric(6, 4))
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::TextEdit::multiline(text)
                                                .id_salt("spatial_comment_text")
                                                .frame(false)
                                                .desired_width(f32::INFINITY)
                                                .desired_rows(2)
                                                .hint_text("What should change here?")
                                                .return_key(egui::KeyboardShortcut::new(
                                                    egui::Modifiers::SHIFT,
                                                    egui::Key::Enter,
                                                )),
                                        )
                                    })
                                    .inner;

                                if !*focused {
                                    response.request_focus();
                                    *focused = true;
                                }

                                let has_text = !text.trim().is_empty();
                                let enter = response.has_focus()
                                    && ui.input_mut(|input| {
                                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                                    });
                                if enter && has_text {
                                    action = OverlayAction::Submit {
                                        text: text.trim().to_string(),
                                        anchors: anchors.clone(),
                                    };
                                }

                                ui.horizontal(|ui| {
                                    theme::key_hint(ui, "Enter");
                                    ui.label(
                                        egui::RichText::new("send")
                                            .small()
                                            .color(theme::TEXT_MUTED),
                                    );
                                    theme::key_hint(ui, "Esc");
                                    ui.label(
                                        egui::RichText::new("cancel")
                                            .small()
                                            .color(theme::TEXT_MUTED),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add_enabled(
                                                    has_text,
                                                    egui::Button::new(
                                                        egui::RichText::new("Send")
                                                            .color(theme::TEXT_STRONG),
                                                    )
                                                    .fill(theme::ACCENT.gamma_multiply(0.55)),
                                                )
                                                .clicked()
                                            {
                                                action = OverlayAction::Submit {
                                                    text: text.trim().to_string(),
                                                    anchors: anchors.clone(),
                                                };
                                            }
                                        },
                                    );
                                });

                                // Escape cancels.
                                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                    action = OverlayAction::Cancel;
                                }
                            });
                    });

                // Connecting line from the card's nearest edge to the anchor.
                let card = area.response.rect;
                let attach = egui::pos2(
                    anchor_pos.x.clamp(card.left(), card.right()),
                    anchor_pos.y.clamp(card.top(), card.bottom()),
                );
                let painter = ui.painter();
                painter.line_segment(
                    [anchor_pos, attach],
                    egui::Stroke::new(1.5_f32, theme::SPATIAL),
                );
                painter.circle_filled(anchor_pos, 4.0, theme::SPATIAL);
                painter.circle_stroke(anchor_pos, 6.0, egui::Stroke::new(1.0_f32, theme::SPATIAL));

                action
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use cadmark_core::geometry::{FaceId, GeometryContext, TopologyElement};
    use cadmark_core::ledger::{
        LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
    };

    use super::{OverlayState, context_summary};

    #[test]
    fn overlay_summary_reads_as_plain_language() {
        let mut identification = std::collections::HashMap::new();
        identification.insert("surface".to_string(), "plane".to_string());
        let context = GeometryContext {
            element: TopologyElement::Face(FaceId(2)),
            provenance: LedgerValue::Resolved(ProvenanceEntry {
                source: SourceRef {
                    line: 4,
                    code: "Box(20, 15, 10)".into(),
                },
                operation: SemanticOperation::Box,
                operation_id: 1,
                relation: ProvenanceRelation::Generated,
            }),
            identification,
            chosen_candidate: None,
        };

        assert_eq!(
            context_summary(&context),
            "face 2: created by box at line 4 (plane)"
        );
    }

    #[test]
    fn clicking_more_geometry_adds_anchors_and_clicking_again_removes_them() {
        use cadmark_core::geometry::{EdgeId, ScreenPosition};
        let anchor = |element: TopologyElement| GeometryContext {
            element,
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            chosen_candidate: None,
        };
        let mut overlay = OverlayState::default();
        assert!(!overlay.toggle_anchor(anchor(TopologyElement::Face(FaceId(1)))));
        overlay.open(
            ScreenPosition { x: 1.0, y: 2.0 },
            anchor(TopologyElement::Face(FaceId(1))),
        );
        assert!(overlay.toggle_anchor(anchor(TopologyElement::Edge(EdgeId(4)))));
        assert_eq!(overlay.anchors().len(), 2);
        overlay.toggle_anchor(anchor(TopologyElement::Edge(EdgeId(4))));
        assert_eq!(overlay.anchors().len(), 1);
        // The last anchor cannot be removed: a comment needs one.
        overlay.toggle_anchor(anchor(TopologyElement::Face(FaceId(1))));
        assert_eq!(overlay.anchors().len(), 1);
    }

    #[test]
    fn overlay_summary_admits_an_untraced_source() {
        let context = GeometryContext {
            element: TopologyElement::Face(FaceId(0)),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            chosen_candidate: None,
        };
        assert!(context_summary(&context).starts_with("face 0: no source line"));
    }

    /// Lay the candidate rows out with the pointer at `pointer`, returning
    /// which candidate the overlay published as hovered.
    fn hover_at(
        ctx: &egui::Context,
        anchors: &mut [GeometryContext],
        pointer: egui::Pos2,
    ) -> Option<ProvenanceEntry> {
        let mut hovered = None;
        for _ in 0..2 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 600.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pointer));
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    super::show_candidate_choices(ui, anchors, &mut hovered);
                });
            });
        }
        hovered
    }

    /// The first pointer y at which each candidate's row is hovered, so the
    /// interaction tests do not depend on egui's exact row metrics.
    fn row_positions(
        ctx: &egui::Context,
        anchors: &mut [GeometryContext],
        candidates: &[ProvenanceEntry],
    ) -> Vec<egui::Pos2> {
        candidates
            .iter()
            .map(|candidate| {
                (0..600)
                    .map(|y| egui::pos2(20.0, y as f32))
                    .find(|pointer| hover_at(ctx, anchors, *pointer).as_ref() == Some(candidate))
                    .unwrap_or_else(|| {
                        panic!("no row of the overlay publishes {candidate:?} on hover")
                    })
            })
            .collect()
    }

    fn click_at(ctx: &egui::Context, anchors: &mut [GeometryContext], pointer: egui::Pos2) {
        let mut hovered = None;
        for pressed in [true, false] {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 600.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pointer));
            input.events.push(egui::Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    super::show_candidate_choices(ui, anchors, &mut hovered);
                });
            });
        }
    }

    fn ambiguous_anchor() -> (GeometryContext, Vec<ProvenanceEntry>) {
        let candidate =
            |line: u32, operation_id: u64, relation: ProvenanceRelation| ProvenanceEntry {
                source: SourceRef {
                    line,
                    code: format!("line {line}"),
                },
                operation: SemanticOperation::Box,
                operation_id,
                relation,
            };
        let candidates = vec![
            candidate(2, 1, ProvenanceRelation::Generated),
            candidate(7, 2, ProvenanceRelation::GeneratedDescendant),
        ];
        (
            GeometryContext {
                element: TopologyElement::Face(FaceId(2)),
                provenance: LedgerValue::Ambiguous(candidates.clone()),
                identification: Default::default(),
                chosen_candidate: None,
            },
            candidates,
        )
    }

    #[test]
    fn hovering_a_candidate_row_publishes_that_candidate_and_no_other() {
        let ctx = egui::Context::default();
        let (context, candidates) = ambiguous_anchor();
        let mut anchors = vec![context];

        let rows = row_positions(&ctx, &mut anchors, &candidates);

        // Distinct rows, so the viewport and code panel can show one
        // candidate's origin at a time rather than a merged set.
        assert_ne!(rows[0], rows[1]);
        assert_eq!(
            hover_at(&ctx, &mut anchors, rows[0]).as_ref(),
            Some(&candidates[0])
        );
        assert_eq!(
            hover_at(&ctx, &mut anchors, rows[1]).as_ref(),
            Some(&candidates[1])
        );
        assert_eq!(
            hover_at(&ctx, &mut anchors, egui::pos2(20.0, 590.0)),
            None,
            "the pointer away from every row must publish no candidate"
        );
    }

    #[test]
    fn choosing_a_candidate_sets_the_anchor_choice_and_choosing_it_again_clears_it() {
        let ctx = egui::Context::default();
        let (context, candidates) = ambiguous_anchor();
        let mut anchors = vec![context];

        let rows = row_positions(&ctx, &mut anchors, &candidates);
        assert_eq!(anchors[0].chosen_candidate, None);

        click_at(&ctx, &mut anchors, rows[1]);
        assert_eq!(
            anchors[0].chosen_candidate.as_ref(),
            Some(&candidates[1]),
            "clicking a row must record it as the line to send alone"
        );

        click_at(&ctx, &mut anchors, rows[0]);
        assert_eq!(anchors[0].chosen_candidate.as_ref(), Some(&candidates[0]));

        click_at(&ctx, &mut anchors, rows[0]);
        assert_eq!(
            anchors[0].chosen_candidate, None,
            "clicking the chosen row again must return to sending every candidate"
        );
    }
}
