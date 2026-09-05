//! Project reference previews displayed independently of the conversation.

pub struct ReferenceImageView<'a> {
    pub name: &'a str,
    pub thumbnail: &'a egui::ColorImage,
}

#[derive(Default)]
pub struct ReferenceImagesPanel {
    textures: Vec<egui::TextureHandle>,
}

impl ReferenceImagesPanel {
    /// Returns true when the user asks to attach images.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        images: &[ReferenceImageView<'_>],
        pending: bool,
    ) -> bool {
        let mut attach = false;
        ui.horizontal(|ui| {
            ui.strong("Project references");
            attach = ui
                .add_enabled(!pending, egui::Button::new("Attach images…"))
                .on_hover_text("Copy PNG or JPEG images into this project for every conversation")
                .clicked();
        });
        if pending {
            ui.label("Choosing or importing images…");
        }
        if images.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Photos and drawings stay with this project across conversations.",
                )
                .small()
                .color(crate::theme::TEXT_MUTED),
            );
        }
        egui::ScrollArea::horizontal()
            .id_salt("project_references")
            .max_height(112.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, image) in images.iter().enumerate() {
                        if self.textures.len() <= index {
                            self.textures.push(ui.ctx().load_texture(
                                format!("reference/{index}/{}", image.name),
                                image.thumbnail.clone(),
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Image::new(&self.textures[index])
                                    .max_size(egui::vec2(96.0, 72.0)),
                            )
                            .on_hover_text(image.name);
                            ui.add(
                                egui::Label::new(egui::RichText::new(image.name).small())
                                    .truncate(),
                            )
                            .on_hover_text(image.name);
                        });
                    }
                });
            });
        attach
    }
}
