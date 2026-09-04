// Comment overlay — appears near selected geometry with a connecting line.
//
// Positioned in screen space near the first selection point and kept
// inside the viewport. The user writes free text and submits with Enter;
// Escape cancels. Clicking more geometry while the overlay is open adds
// anchors, so one comment can point at several elements.

use cadmark_core::geometry::{GeometryContext, ScreenPosition};

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
            sketch: Default::default(),
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
            source_context: String::new(),
            neighbours: Vec::new(),
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
            sketch: Default::default(),
            element,
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
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
            sketch: Default::default(),
            element: TopologyElement::Face(FaceId(0)),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
        };
        assert!(context_summary(&context).starts_with("face 0: no source line"));
    }
}
