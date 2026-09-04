//! Reference-image controls shown above the conversation.
//!
//! The application owns the project folder and file picker. This panel only
//! renders the images already attached to that folder and reports an attach
//! request, keeping egui separate from project storage.

/// One image ready for a compact preview in the chat pane.
#[derive(Debug, Clone)]
pub struct ReferenceImageView<'a> {
    /// Stable path-based identity for the texture cache.
    pub id: String,
    pub name: &'a str,
    pub bytes: &'a [u8],
}

/// What the user did in the reference-image panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceImagesAction {
    None,
    Attach,
}

/// State for the project reference-image panel.
#[derive(Default)]
pub struct ReferenceImagesPanel {
    previews: std::collections::BTreeMap<String, egui::TextureHandle>,
}

impl ReferenceImagesPanel {
    /// Draw attached-image previews and offer the system picker.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        images: &[ReferenceImageView<'_>],
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

        self.previews
            .retain(|id, _| images.iter().any(|image| image.id == *id));

        egui::ScrollArea::horizontal()
            .id_salt("reference_images")
            .max_height(94.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for image in images {
                        ui.vertical(|ui| {
                            if let Some(texture) = self.preview(ui, image) {
                                ui.add(
                                    egui::Image::new(&texture)
                                        .fit_to_exact_size(egui::vec2(64.0, 64.0)),
                                )
                                .on_hover_text(image.name);
                            } else {
                                ui.label(
                                    egui::RichText::new("Image could not be previewed")
                                        .small()
                                        .color(crate::theme::ERROR),
                                );
                            }
                            ui.label(
                                egui::RichText::new(image.name)
                                    .small()
                                    .color(crate::theme::TEXT_MUTED),
                            );
                        });
                    }
                });
            });
        action
    }

    fn preview(
        &mut self,
        ui: &egui::Ui,
        image: &ReferenceImageView<'_>,
    ) -> Option<egui::TextureHandle> {
        if let Some(texture) = self.previews.get(&image.id) {
            return Some(texture.clone());
        }
        let thumbnail = decode_thumbnail(image.bytes)?;
        let texture = ui.ctx().load_texture(
            format!("cadmark-reference-image/{}", image.id),
            thumbnail,
            egui::TextureOptions::LINEAR,
        );
        self.previews.insert(image.id.clone(), texture.clone());
        Some(texture)
    }
}

/// Decode at most the preview's texture budget; egui only ever receives this
/// compact image, not the original photograph's full-resolution pixels.
fn decode_thumbnail(bytes: &[u8]) -> Option<egui::ColorImage> {
    const THUMBNAIL_EDGE: u32 = 96;
    let thumbnail = image::load_from_memory(bytes)
        .ok()?
        .thumbnail(THUMBNAIL_EDGE, THUMBNAIL_EDGE)
        .to_rgba8();
    let size = [thumbnail.width() as usize, thumbnail.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        thumbnail.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    #[test]
    fn thumbnails_are_bounded_before_texture_upload() {
        let source = image::RgbaImage::from_pixel(384, 192, image::Rgba([0, 128, 255, 255]));
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();

        let thumbnail = super::decode_thumbnail(bytes.get_ref()).unwrap();

        assert_eq!(thumbnail.size, [96, 48]);
    }
}
