// Comment overlay — appears near selected geometry with a connecting line.
//
// Positioned in screen space near the first selection point and kept
// inside the viewport. The user writes free text and submits with Enter;
// Escape cancels. Clicking more geometry while the overlay is open adds
// anchors, so one comment can point at several elements.

use cadmark_core::candidates::order_candidates;
use cadmark_core::geometry::{GeometryContext, PartId, PickedElement, ScreenPosition};
use cadmark_core::ledger::{LedgerValue, ProvenanceEntry};
use cadmark_core::sketch_lineage::NoSketchRoute;

use crate::theme;

/// One line about an anchor: its element, where it came from, and its
/// surface or curve type. A sketch element's origin is the line that drew
/// it — it has no ledger provenance because nothing the kernel built claims
/// a drawn curve — so its sketch route stands in for the provenance.
fn context_summary(context: &GeometryContext) -> String {
    let origin = match &context.element {
        PickedElement::Sketch(_) => context.sketch.describe(),
        PickedElement::Solid(_) => context.provenance.describe(),
    };
    let mut summary = format!("{}: {origin}", context.element.display_label());
    if let Some(surface) = context
        .identification
        .get("surface")
        .or_else(|| context.identification.get("curve"))
    {
        summary.push_str(&format!(" ({surface})"));
    }
    summary
}

/// The sketch route of a solid element, offered beneath its own origin so
/// a comment on an extruded boss's side face can reach the profile line
/// that drew it. Each drawn curve is a row; resting the pointer on one
/// shows its line in the code panel. A sketch element's route is its
/// origin and is already in the summary; an element with no route says
/// why, in the ledger's own words.
fn show_sketch_routes(
    ui: &mut egui::Ui,
    anchors: &[GeometryContext],
    hovered_sketch_line: &mut Option<u32>,
) {
    *hovered_sketch_line = None;
    for context in anchors {
        let PickedElement::Solid(_) = &context.element else {
            continue;
        };
        let sources = context.sketch.candidates();
        if sources.is_empty() {
            if let Some(reason) = context.sketch.no_route()
                && !matches!(reason, NoSketchRoute::NoSketchAncestor)
            {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{}: {}",
                            context.element.display_label(),
                            reason.describe()
                        ))
                        .small()
                        .color(theme::TEXT_MUTED),
                    )
                    .wrap(),
                );
            }
            continue;
        }
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{} comes from the sketch{}:",
                    context.element.display_label(),
                    if sources.len() > 1 {
                        " (several curves reach it)"
                    } else {
                        ""
                    }
                ))
                .small()
                .color(theme::TEXT_MUTED),
            )
            .wrap(),
        );
        for source in sources {
            let row = ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{}: `{}`", source.describe(), source.source.code))
                        .small(),
                )
                .sense(egui::Sense::hover()),
            );
            if row.hovered() {
                *hovered_sketch_line = Some(source.source.line);
            }
        }
    }
}

/// What the candidate list says about its own order.
///
/// The ranked and unranked cases must read differently: an order the
/// ledger did not determine, presented as though it had, is the guess the
/// provenance design exists to refuse.
fn candidate_heading(label: &str, count: usize, ranked: bool) -> String {
    format!(
        "{label} could have come from {count} lines. {}",
        if ranked {
            "Most likely first."
        } else {
            "CADmark cannot tell which is likeliest; these are in the order it recorded them."
        }
    )
}

/// What will actually be sent to the AI for this anchor, stated plainly so
/// the user can see the consequence of choosing and of not choosing.
fn sending_note(chosen: Option<&ProvenanceEntry>, count: usize) -> String {
    match chosen {
        Some(chosen) => format!("Sending line {} alone.", chosen.source.line),
        None => format!("No line chosen: sending all {count} for the AI to decide."),
    }
}

/// A candidate row the pointer is over, with the part its anchor is
/// numbered within, so the viewport lights that part's geometry rather
/// than whichever part was clicked last.
#[derive(Debug, Clone, PartialEq)]
pub struct HoveredCandidate {
    pub part: Option<PartId>,
    pub entry: ProvenanceEntry,
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
    hovered_candidate: &mut Option<HoveredCandidate>,
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
                egui::RichText::new(candidate_heading(
                    &context.element.display_label(),
                    candidates.len(),
                    ordering.ranked,
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
                *hovered_candidate = Some(HoveredCandidate {
                    part: context.part,
                    entry: candidate.clone(),
                });
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
                egui::RichText::new(sending_note(
                    context.chosen_candidate.as_ref(),
                    candidates.len(),
                ))
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
        hovered_candidate: Option<HoveredCandidate>,
        /// The sketch-route row the pointer is over, recomputed each frame
        /// so the code panel can show the drawing line it names.
        hovered_sketch_line: Option<u32>,
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
            hovered_sketch_line: None,
        };
    }

    /// Add an anchor to the open comment; clicking an element already
    /// anchored removes it. An element is the same one only within the
    /// same part, since every part numbers its own elements from zero.
    /// Returns whether the overlay was open.
    pub fn toggle_anchor(&mut self, context: GeometryContext) -> bool {
        let Self::Active { anchors, .. } = self else {
            return false;
        };
        match anchors
            .iter()
            .position(|anchor| anchor.part == context.part && anchor.element == context.element)
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
    pub fn hovered_candidate(&self) -> Option<&HoveredCandidate> {
        match self {
            Self::Active {
                hovered_candidate, ..
            } => hovered_candidate.as_ref(),
            Self::Hidden => None,
        }
    }

    /// The sketch-route line the pointer is resting on, if any, for the
    /// code panel to bring into view.
    pub fn hovered_sketch_line(&self) -> Option<u32> {
        match self {
            Self::Active {
                hovered_sketch_line,
                ..
            } => *hovered_sketch_line,
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
                hovered_sketch_line,
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
                                // The caption and close button take a row
                                // of their own; the anchors tessellate
                                // beneath them, however many there are.
                                ui.horizontal(|ui| {
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
                                let chips: Vec<theme::ChipEntry> = anchors
                                    .iter()
                                    .map(|context| theme::ChipEntry {
                                        label: context.element.display_label(),
                                        hover: Some(context_summary(context)),
                                    })
                                    .collect();
                                theme::chip_grid(ui, &chips, theme::SPATIAL);
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
                                show_sketch_routes(ui, anchors, hovered_sketch_line);

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
                                let enter = crate::text_input::consume_submit(ui, &response);
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
    use cadmark_core::geometry::{FaceId, GeometryContext, PartId, PickedElement, TopologyElement};
    use cadmark_core::ledger::{
        LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
    };

    use super::{HoveredCandidate, OverlayState, context_summary};

    #[test]
    fn overlay_summary_reads_as_plain_language() {
        let mut identification = std::collections::HashMap::new();
        identification.insert("surface".to_string(), "plane".to_string());
        let context = GeometryContext {
            part: None,
            element: PickedElement::Solid(TopologyElement::Face(FaceId(2))),
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
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
            sketch: Default::default(),
        };

        assert_eq!(
            context_summary(&context),
            "face 2: created by box at line 4 (plane)"
        );
    }

    #[test]
    fn the_same_element_number_on_two_parts_is_two_anchors() {
        use cadmark_core::geometry::ScreenPosition;
        let anchor = |part: u32| GeometryContext {
            part: Some(PartId(part)),
            element: PickedElement::Solid(TopologyElement::Face(FaceId(0))),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: vec![],
            chosen_candidate: None,
            sketch: Default::default(),
        };
        let mut overlay = OverlayState::default();
        overlay.open(ScreenPosition { x: 0.0, y: 0.0 }, anchor(1));
        assert!(overlay.toggle_anchor(anchor(2)));
        assert_eq!(
            overlay.anchors().len(),
            2,
            "face 0 of another part is a new anchor"
        );
        assert!(overlay.toggle_anchor(anchor(2)));
        assert_eq!(
            overlay.anchors().len(),
            1,
            "clicking it again removes only itself"
        );
        assert_eq!(overlay.anchors()[0].part, Some(PartId(1)));
    }

    #[test]
    fn clicking_more_geometry_adds_anchors_and_clicking_again_removes_them() {
        use cadmark_core::geometry::{EdgeId, ScreenPosition};
        let anchor = |element: TopologyElement| GeometryContext {
            part: None,
            element: PickedElement::Solid(element),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
            sketch: Default::default(),
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

    /// A sketch element's origin is the line that drew it, which the
    /// summary states in place of the ledger provenance it cannot have.
    #[test]
    fn overlay_summary_names_the_drawing_line_of_a_sketch_element() {
        use cadmark_core::geometry::{SketchElement, SketchElementKind};
        use cadmark_core::sketch_lineage::{SketchLineage, SketchSource};
        let mut identification = std::collections::HashMap::new();
        identification.insert("curve".to_string(), "circle".to_string());
        let context = GeometryContext {
            part: None,
            element: PickedElement::Sketch(SketchElement {
                kind: SketchElementKind::Curve,
                index: 3,
            }),
            provenance: LedgerValue::Untraced,
            identification,
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
            sketch: SketchLineage::Resolved(SketchSource {
                source: SourceRef {
                    line: 6,
                    code: "Circle(5)".into(),
                },
                object: "Circle".to_string(),
            }),
        };
        assert_eq!(
            context_summary(&context),
            "sketch curve 3: drawn by Circle at line 6 (circle)"
        );
    }

    /// A solid element with a sketch ancestor offers the drawing line as a
    /// row; resting the pointer on it publishes that line. An element with
    /// no sketch anywhere in its construction offers nothing.
    #[test]
    fn a_solid_elements_sketch_route_is_offered_and_hoverable() {
        use cadmark_core::sketch_lineage::{SketchLineage, SketchSource};
        let anchor = |sketch: SketchLineage| GeometryContext {
            part: None,
            element: PickedElement::Solid(TopologyElement::Face(FaceId(5))),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
            sketch,
        };
        let routed = anchor(SketchLineage::Resolved(SketchSource {
            source: SourceRef {
                line: 9,
                code: "Rectangle(40, 20)".into(),
            },
            object: "Rectangle".to_string(),
        }));
        let ctx = egui::Context::default();
        let hover = |anchors: &[GeometryContext], pointer: egui::Pos2| {
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
                        super::show_sketch_routes(ui, anchors, &mut hovered);
                    });
                });
            }
            hovered
        };
        let found = (0..600)
            .map(|y| egui::pos2(20.0, y as f32))
            .find_map(|pointer| hover(std::slice::from_ref(&routed), pointer));
        assert_eq!(found, Some(9), "the drawing line's row is hoverable");

        let unrouted = anchor(SketchLineage::default());
        assert!(
            (0..600)
                .map(|y| egui::pos2(20.0, y as f32))
                .all(|pointer| hover(std::slice::from_ref(&unrouted), pointer).is_none()),
            "an element with no sketch ancestor offers no route"
        );
    }

    #[test]
    fn overlay_summary_admits_an_untraced_source() {
        let context = GeometryContext {
            part: None,
            element: PickedElement::Solid(TopologyElement::Face(FaceId(0))),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
            sketch: Default::default(),
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
        hovered_at(ctx, anchors, pointer).map(|hovered| hovered.entry)
    }

    /// As `hover_at`, with the part the hovered row's anchor belongs to.
    fn hovered_at(
        ctx: &egui::Context,
        anchors: &mut [GeometryContext],
        pointer: egui::Pos2,
    ) -> Option<HoveredCandidate> {
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
                part: None,
                element: PickedElement::Solid(TopologyElement::Face(FaceId(2))),
                provenance: LedgerValue::Ambiguous(candidates.clone()),
                identification: Default::default(),
                source_context: String::new(),
                neighbours: Vec::new(),
                chosen_candidate: None,
                sketch: Default::default(),
            },
            candidates,
        )
    }

    #[test]
    fn an_order_the_ledger_could_not_determine_is_offered_as_no_ranking() {
        // Two candidates sharing a construction step: the ledger has
        // nothing to rank on, and the heading must not imply it has.
        let tied = [
            ProvenanceEntry {
                source: SourceRef {
                    line: 4,
                    code: "line 4".into(),
                },
                operation: SemanticOperation::Box,
                operation_id: 3,
                relation: ProvenanceRelation::Generated,
            },
            ProvenanceEntry {
                source: SourceRef {
                    line: 8,
                    code: "line 8".into(),
                },
                operation: SemanticOperation::Box,
                operation_id: 3,
                relation: ProvenanceRelation::Modified,
            },
        ];
        let ordering = cadmark_core::candidates::order_candidates(&tied);
        let heading = super::candidate_heading("edge 5", tied.len(), ordering.ranked);
        assert!(
            heading.contains("cannot tell which is likeliest"),
            "an undetermined order must reach the user as one: {heading}"
        );
        assert!(!heading.contains("Most likely first"));

        // And the ranked case must say the opposite, or the sentence
        // carries no information either way.
        let (_, ranked_candidates) = ambiguous_anchor();
        let ranked = cadmark_core::candidates::order_candidates(&ranked_candidates);
        assert!(ranked.ranked);
        let heading = super::candidate_heading("edge 5", ranked_candidates.len(), ranked.ranked);
        assert!(heading.contains("Most likely first"), "{heading}");
    }

    #[test]
    fn the_overlay_states_whether_one_line_or_every_line_will_be_sent() {
        let (_, candidates) = ambiguous_anchor();
        assert_eq!(
            super::sending_note(None, candidates.len()),
            "No line chosen: sending all 2 for the AI to decide."
        );
        assert_eq!(
            super::sending_note(Some(&candidates[1]), candidates.len()),
            "Sending line 7 alone."
        );
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
    fn a_hovered_candidate_names_the_part_its_anchor_is_on() {
        // Two anchors on two parts, each ambiguous. Hovering a row under
        // the second anchor must publish the second part, whichever part
        // the user clicked last, so the viewport lights the right solid.
        let ctx = egui::Context::default();
        let (context, candidates) = ambiguous_anchor();
        let mut on_first = context.clone();
        on_first.part = Some(PartId(1));
        let mut on_second = context;
        on_second.part = Some(PartId(2));
        let mut anchors = vec![on_first, on_second];

        let rows: Vec<_> = (0..600)
            .map(|y| egui::pos2(20.0, y as f32))
            .filter_map(|pointer| hovered_at(&ctx, &mut anchors, pointer))
            .fold(Vec::new(), |mut rows: Vec<HoveredCandidate>, hovered| {
                if rows.last() != Some(&hovered) {
                    rows.push(hovered);
                }
                rows
            });
        assert_eq!(
            rows,
            vec![
                HoveredCandidate {
                    part: Some(PartId(1)),
                    entry: candidates[0].clone()
                },
                HoveredCandidate {
                    part: Some(PartId(1)),
                    entry: candidates[1].clone()
                },
                HoveredCandidate {
                    part: Some(PartId(2)),
                    entry: candidates[0].clone()
                },
                HoveredCandidate {
                    part: Some(PartId(2)),
                    entry: candidates[1].clone()
                },
            ],
            "each anchor's rows carry that anchor's part"
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
