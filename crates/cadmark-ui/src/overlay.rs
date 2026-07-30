// Comment overlay — appears near selected geometry with a connecting line.
//
// Positioned in screen space near the selection point. The user writes
// free text and submits. Escape cancels and returns to default state.

use cadmark_core::geometry::{GeometryContext, ScreenPosition};

fn context_summary(context: &GeometryContext) -> String {
    format!(
        "{:?} · {:?} · line {} · {:?}",
        context.element,
        context.provenance.operation,
        context.provenance.source.line,
        context.provenance.relation
    )
}

/// State for the spatial comment overlay.
#[derive(Debug)]
pub enum OverlayState {
    /// No overlay visible.
    Hidden,
    /// Overlay is open at a screen position, user is writing.
    Active {
        /// Where the selected element is on screen.
        anchor: ScreenPosition,
        /// The text the user is writing.
        text: String,
        /// Provenance resolved at pick time and retained through submission.
        context: GeometryContext,
    },
}

impl Default for OverlayState {
    fn default() -> Self {
        Self::Hidden
    }
}

/// Result of showing the overlay — what action the user took.
pub enum OverlayAction {
    /// No action — overlay still open or hidden.
    None,
    /// User submitted a spatial comment.
    Submit {
        text: String,
        context: GeometryContext,
    },
    /// User cancelled (Escape).
    Cancel,
}

impl OverlayState {
    /// Open the overlay at the given screen position.
    pub fn open(&mut self, anchor: ScreenPosition, context: GeometryContext) {
        *self = Self::Active {
            anchor,
            text: String::new(),
            context,
        };
    }

    /// Close the overlay.
    pub fn close(&mut self) {
        *self = Self::Hidden;
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    /// Render the overlay. Returns the action the user took.
    pub fn show(&mut self, ui: &mut egui::Ui) -> OverlayAction {
        match self {
            Self::Hidden => OverlayAction::None,
            Self::Active {
                anchor,
                text,
                context,
            } => {
                let mut action = OverlayAction::None;

                // Position the overlay near the anchor with an offset.
                let overlay_pos = egui::pos2(anchor.x + 20.0, anchor.y - 10.0);

                egui::Area::new(egui::Id::new("spatial_comment_overlay"))
                    .fixed_pos(overlay_pos)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_min_width(200.0);
                            ui.label(
                                egui::RichText::new("Spatial Comment")
                                    .strong()
                                    .color(egui::Color32::from_rgb(100, 200, 255)),
                            );
                            ui.small(context_summary(context));

                            let response = ui.text_edit_multiline(text);

                            // Submit on Ctrl+Enter.
                            if response.has_focus()
                                && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter))
                                && !text.trim().is_empty()
                            {
                                action = OverlayAction::Submit {
                                    text: text.trim().to_string(),
                                    context: context.clone(),
                                };
                            }

                            ui.horizontal(|ui| {
                                if ui.button("Submit").clicked() && !text.trim().is_empty() {
                                    action = OverlayAction::Submit {
                                        text: text.trim().to_string(),
                                        context: context.clone(),
                                    };
                                }
                                if ui.button("Cancel").clicked() {
                                    action = OverlayAction::Cancel;
                                }
                            });

                            // Escape cancels.
                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                action = OverlayAction::Cancel;
                            }
                        });
                    });

                // Draw connecting line from overlay to anchor.
                let painter = ui.painter();
                painter.line_segment(
                    [egui::pos2(anchor.x, anchor.y), overlay_pos],
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(100, 200, 255)),
                );

                action
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use cadmark_core::geometry::{FaceId, GeometryContext, TopologyElement};
    use cadmark_core::ledger::{ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef};

    use super::context_summary;

    #[test]
    fn overlay_summary_exposes_the_retained_resolved_context() {
        let context = GeometryContext {
            element: TopologyElement::Face(FaceId(2)),
            provenance: ProvenanceEntry {
                source: SourceRef {
                    line: 4,
                    code: "Box(20, 15, 10)".into(),
                },
                operation: SemanticOperation::Box,
                operation_id: 1,
                relation: ProvenanceRelation::Generated,
            },
            identification: Default::default(),
        };

        assert_eq!(
            context_summary(&context),
            "Face(FaceId(2)) · Box · line 4 · Generated"
        );
    }
}
