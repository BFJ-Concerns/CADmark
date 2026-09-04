//! Reference-image controls shown above the conversation.
//!
//! The application owns the project folder and file picker. This panel only
//! renders the images already attached to that folder and reports an attach
//! request, keeping egui separate from project storage.

/// One image ready for a compact preview in the chat pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceImageView {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// What the user did in the reference-image panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceImagesAction {
    None,
    Attach,
}

/// State for the project reference-image panel.
#[derive(Debug, Default)]
pub struct ReferenceImagesPanel;

impl ReferenceImagesPanel {
    /// Draw attached-image previews and offer the system picker.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        images: &[ReferenceImageView],
    ) -> ReferenceImagesAction {
        let mut action = ReferenceImagesAction::None;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Reference images").strong());
            if ui.button("Attach image").clicked() {
                action = ReferenceImagesAction::Attach;
            }
        });
        if images.is_empty() {
            return action;
        }

        egui::ScrollArea::horizontal()
            .id_salt("reference_images")
            .max_height(94.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for image in images {
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Image::from_bytes(
                                    format!("bytes://cadmark-reference/{}", image.name),
                                    image.bytes.clone(),
                                )
                                .fit_to_exact_size(egui::vec2(64.0, 64.0)),
                            )
                            .on_hover_text(&image.name);
                            ui.label(
                                egui::RichText::new(&image.name)
                                    .small()
                                    .color(crate::theme::TEXT_MUTED),
                            );
                        });
                    }
                });
            });
        action
    }
}
