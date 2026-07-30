// Chat pane — displays conversation with message type distinction.
//
// Three visual types: user chat (plain), spatial comments (with
// geometry chip header), and AI responses. Applied spatial comments
// are shown in a muted state.

use cadmark_core::message::{Conversation, MessageKind};

/// State for the chat pane UI.
#[derive(Debug, Default)]
pub struct ChatPane {
    /// Current text in the input field.
    pub input_text: String,
    /// Whether the AI is currently generating a response.
    pub is_loading: bool,
    /// Whether to auto-scroll to the bottom on new messages.
    pub auto_scroll: bool,
}

impl ChatPane {
    pub fn new() -> Self {
        Self {
            input_text: String::new(),
            is_loading: false,
            auto_scroll: true,
        }
    }

    /// Render the chat pane. Returns Some(text) if the user submitted a message.
    pub fn show(&mut self, ui: &mut egui::Ui, conversation: &Conversation) -> Option<String> {
        let mut submitted = None;

        // Reserve space for the input area at the bottom first,
        // so the scroll area doesn't greedily consume everything.
        let input_height = 28.0 + ui.spacing().item_spacing.y + 2.0;
        let separator_height = ui.spacing().item_spacing.y * 2.0 + 1.0;
        let bottom_reserve = input_height + separator_height;

        let available = ui.available_height();
        let scroll_height = (available - bottom_reserve).max(40.0);

        // Message list.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(self.auto_scroll)
            .max_height(scroll_height)
            .show(ui, |ui| {
                for message in conversation.messages() {
                    match &message.kind {
                        MessageKind::UserChat => {
                            ui.group(|ui| {
                                ui.label(
                                    egui::RichText::new("You")
                                        .strong()
                                        .color(egui::Color32::from_rgb(180, 180, 220)),
                                );
                                ui.label(&message.text);
                            });
                        }
                        MessageKind::SpatialComment { context, applied } => {
                            let alpha = if *applied { 100 } else { 255 };
                            ui.group(|ui| {
                                // Geometry chip header.
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Spatial Comment").strong().color(
                                            egui::Color32::from_rgba_premultiplied(
                                                100, 200, 255, alpha,
                                            ),
                                        ),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "line {}",
                                            context.provenance.source.line
                                        ))
                                        .small()
                                        .color(
                                            egui::Color32::from_rgba_premultiplied(
                                                150, 150, 150, alpha,
                                            ),
                                        ),
                                    );
                                    if *applied {
                                        ui.label(
                                            egui::RichText::new("Applied")
                                                .small()
                                                .italics()
                                                .color(egui::Color32::from_rgb(120, 120, 120)),
                                        );
                                    }
                                });
                                ui.label(egui::RichText::new(&message.text).color(
                                    egui::Color32::from_rgba_premultiplied(220, 220, 220, alpha),
                                ));
                            });
                        }
                        MessageKind::AiResponse => {
                            ui.group(|ui| {
                                ui.label(
                                    egui::RichText::new("CADmark")
                                        .strong()
                                        .color(egui::Color32::from_rgb(130, 220, 130)),
                                );
                                ui.label(&message.text);
                            });
                        }
                    }
                    ui.add_space(4.0);
                }

                // Loading indicator.
                if self.is_loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Generating...")
                                .italics()
                                .color(egui::Color32::from_rgb(150, 150, 150)),
                        );
                    });
                }
            });

        ui.separator();

        // Input area.
        ui.horizontal(|ui| {
            let response = ui.add_sized(
                [ui.available_width() - 60.0, 28.0],
                egui::TextEdit::singleline(&mut self.input_text)
                    .hint_text("Describe what you want to build...")
                    .interactive(!self.is_loading),
            );

            let send_clicked = ui
                .add_enabled(!self.is_loading, egui::Button::new("Send"))
                .clicked();

            // Submit on Enter or button click.
            let enter_pressed =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            if (send_clicked || enter_pressed) && !self.input_text.trim().is_empty() {
                submitted = Some(self.input_text.trim().to_string());
                self.input_text.clear();
            }
        });

        submitted
    }
}
