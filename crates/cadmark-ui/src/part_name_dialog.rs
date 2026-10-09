// Part-naming prompt — the first save of a part that has none.
//
// A part made in the application is Untitled until it is saved; the save
// asks what to call it, and the answer becomes its file name in the
// project folder. A floating window, not a modal: the viewport and chat
// stay live while it is open.

use crate::theme;

/// State of the naming prompt.
#[derive(Debug, Default)]
pub struct PartNameDialog {
    open: bool,
    name: String,
    focused: bool,
    /// Why the last name offered was refused.
    rejection: Option<String>,
}

/// What the prompt decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartNameAction {
    None,
    /// The user offered a name for the part.
    Save(String),
    Cancel,
}

impl PartNameDialog {
    pub fn open(&mut self) {
        self.open = true;
        self.name.clear();
        self.focused = false;
        self.rejection = None;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Keep the prompt open with the reason the name was not accepted.
    pub fn reject(&mut self, reason: String) {
        self.open = true;
        self.rejection = Some(reason);
    }

    /// Render the prompt if it is open. Returns the action the user took.
    pub fn show(&mut self, ctx: &egui::Context) -> PartNameAction {
        if !self.open {
            return PartNameAction::None;
        }
        let mut action = PartNameAction::None;

        let mut open = true;
        egui::Window::new("Name this part")
            .id(egui::Id::new("name_part_dialog"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(theme::RAISED)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(
                    egui::RichText::new(
                        "The name you give this part becomes its file name in the project \
                         folder, such as \u{201C}bracket\u{201D} or \u{201C}housing\u{201D}.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                ui.add_space(8.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .id_salt("part_name")
                        .desired_width(f32::INFINITY)
                        .hint_text("Part name"),
                );
                if response.changed() {
                    self.rejection = None;
                }
                if !self.focused {
                    response.request_focus();
                    self.focused = true;
                }
                if let Some(reason) = &self.rejection {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(reason).small().color(theme::ERROR));
                }
                let name = self.name.trim();
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if enter && !name.is_empty() {
                    action = PartNameAction::Save(name.to_string());
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !name.is_empty(),
                                egui::Button::new(
                                    egui::RichText::new("Save part").color(theme::TEXT_STRONG),
                                )
                                .fill(theme::ACCENT.gamma_multiply(0.55)),
                            )
                            .clicked()
                        {
                            action = PartNameAction::Save(name.to_string());
                        }
                        if ui.button("Cancel").clicked() {
                            action = PartNameAction::Cancel;
                        }
                    });
                });
            });

        if !open {
            action = PartNameAction::Cancel;
        }
        // A rejected name keeps the prompt open; `reject` reopens it.
        if action != PartNameAction::None {
            self.open = false;
        }
        action
    }
}
