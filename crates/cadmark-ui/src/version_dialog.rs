// Version-naming dialog — mark the current model as a named version.
//
// A named version is a design step the user chose to label, so it stands
// out in the history and can be returned to by name.

use crate::theme;

/// State of the naming dialog.
#[derive(Debug, Default)]
pub struct VersionDialog {
    open: bool,
    name: String,
    focused: bool,
}

/// What the dialog decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionDialogAction {
    None,
    /// The user confirmed a name.
    Save(String),
    Cancel,
}

impl VersionDialog {
    pub fn open(&mut self) {
        self.open = true;
        self.name.clear();
        self.focused = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Render the dialog if it is open. Returns the action the user took.
    pub fn show(&mut self, ctx: &egui::Context) -> VersionDialogAction {
        if !self.open {
            return VersionDialogAction::None;
        }
        let mut action = VersionDialogAction::None;

        let modal = egui::Modal::new(egui::Id::new("name_version_dialog"))
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(theme::RAISED)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(
                    egui::RichText::new("Name this version")
                        .heading()
                        .color(theme::TEXT_STRONG),
                );
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(
                        "A named version is a design step you can find again by name, \
                         such as \u{201C}Ready for print\u{201D} or \u{201C}Before the ribs\u{201D}.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                ui.add_space(8.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .id_salt("version_name")
                        .desired_width(f32::INFINITY)
                        .hint_text("Version name"),
                );
                if !self.focused {
                    response.request_focus();
                    self.focused = true;
                }
                let name = self.name.trim();
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if enter && !name.is_empty() {
                    action = VersionDialogAction::Save(name.to_string());
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !name.is_empty(),
                                egui::Button::new(
                                    egui::RichText::new("Save version").color(theme::TEXT_STRONG),
                                )
                                .fill(theme::ACCENT.gamma_multiply(0.55)),
                            )
                            .clicked()
                        {
                            action = VersionDialogAction::Save(name.to_string());
                        }
                        if ui.button("Cancel").clicked() {
                            action = VersionDialogAction::Cancel;
                        }
                    });
                });
            });

        if modal.should_close() {
            action = VersionDialogAction::Cancel;
        }
        if action != VersionDialogAction::None {
            self.open = false;
        }
        action
    }
}
