// Settings dialog — the AI provider and the script execution ceilings,
// configured once for every project. Opens from the toolbar and from the
// AI badge when no provider is set; the credential field is masked and
// its value is never echoed back into the form. A floating window, not a
// modal: while it is open the viewport still takes clicks and the chat
// still takes text.

use std::time::Duration;

use cadmark_core::limits::ExecutionLimits;

use crate::theme;

/// What the dialog edits: a copy the user changes, applied on Save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsForm {
    pub base_url: String,
    pub model: String,
    pub accepts_images: bool,
    pub allow_insecure_http: bool,
    /// The reasoning effort asked of the model on every request; blank
    /// sends nothing and leaves the provider's default.
    pub reasoning_effort: String,
    /// A new credential to store; empty leaves the stored one alone.
    pub credential: String,
    /// Whether a credential is already stored, for the field's hint.
    pub has_stored_credential: bool,
    /// Whether the environment variable is set and overrides the store.
    pub credential_from_environment: bool,
    pub wall_clock_seconds: u64,
    pub memory_megabytes: u64,
    pub context_window_tokens: usize,
    /// What the endpoint advertised for the model, when it did: shown so
    /// the user knows the manual figure is the fallback.
    pub detected_context_window: Option<usize>,
}

impl SettingsForm {
    pub fn limits(&self) -> ExecutionLimits {
        ExecutionLimits {
            wall_clock: Duration::from_secs(self.wall_clock_seconds.max(1)),
            memory_bytes: self.memory_megabytes.max(64) * 1024 * 1024,
        }
    }
}

/// What the dialog decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsAction {
    None,
    Save(SettingsForm),
    Cancel,
}

#[derive(Debug, Default)]
pub struct SettingsDialog {
    form: Option<SettingsForm>,
    /// The last save's validation failure, shown until the form changes.
    error: Option<String>,
}

impl SettingsDialog {
    pub fn open(&mut self, form: SettingsForm) {
        self.form = Some(form);
        self.error = None;
    }

    /// The form as it stands while the dialog is open.
    pub fn form(&self) -> Option<&SettingsForm> {
        self.form.as_ref()
    }

    pub fn is_open(&self) -> bool {
        self.form.is_some()
    }

    /// Keep the dialog open and show why the save was refused.
    pub fn reject(&mut self, reason: String) {
        self.error = Some(reason);
    }

    pub fn close(&mut self) {
        self.form = None;
        self.error = None;
    }

    /// Render the dialog if it is open. Returns the action the user took.
    pub fn show(&mut self, ctx: &egui::Context) -> SettingsAction {
        let Some(form) = &mut self.form else {
            return SettingsAction::None;
        };
        let mut action = SettingsAction::None;

        let mut open = true;
        egui::Window::new("Settings")
            .id(egui::Id::new("settings_dialog"))
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
                ui.set_width(460.0);

                ui.label(
                    egui::RichText::new("AI provider")
                        .strong()
                        .color(theme::TEXT_STRONG),
                );
                ui.label(
                    egui::RichText::new(
                        "Any OpenAI-compatible endpoint: OpenAI, a gateway in front of Claude \
                         or Gemini, OpenRouter, or a local model server.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                egui::Grid::new("settings_provider")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Base URL");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.base_url)
                                .hint_text("https://api.openai.com/v1")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();
                        ui.label("Model");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.model)
                                .hint_text("model name")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();
                        ui.label("Reasoning effort");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.reasoning_effort)
                                .hint_text("blank for the provider's default; low, medium, high")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();
                        ui.label("Credential");
                        let hint = if form.credential_from_environment {
                            "set by the environment variable; a value here is stored but not used"
                        } else if form.has_stored_credential {
                            "stored; enter a new value to replace it"
                        } else {
                            "API key"
                        };
                        ui.add(
                            egui::TextEdit::singleline(&mut form.credential)
                                .password(true)
                                .hint_text(hint)
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();
                    });
                ui.checkbox(
                    &mut form.accepts_images,
                    "The model reads images (enables the render and reference-image tools)",
                );
                ui.checkbox(
                    &mut form.allow_insecure_http,
                    "Allow a plain-HTTP endpoint (local servers)",
                );

                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new("Script limits")
                        .strong()
                        .color(theme::TEXT_STRONG),
                );
                ui.label(
                    egui::RichText::new(
                        "A script that runs past either ceiling is stopped and the AI is told.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                egui::Grid::new("settings_limits")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Wall clock (seconds)");
                        ui.add(egui::DragValue::new(&mut form.wall_clock_seconds).range(1..=3600));
                        ui.end_row();
                        ui.label("Memory (MB)");
                        ui.add(egui::DragValue::new(&mut form.memory_megabytes).range(64..=65536));
                        ui.end_row();
                    });

                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new("Conversation context")
                        .strong()
                        .color(theme::TEXT_STRONG),
                );
                ui.label(
                    egui::RichText::new(
                        "Set this to the configured model's context window. CADmark condenses \
                         conversation before it approaches this limit.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                egui::Grid::new("settings_context")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Context window (tokens)");
                        ui.add(
                            egui::DragValue::new(&mut form.context_window_tokens)
                                .range(1_024..=10_000_000),
                        );
                        ui.end_row();
                    });
                ui.label(
                    egui::RichText::new(match form.detected_context_window {
                        Some(detected) => format!(
                            "The endpoint reports {detected} tokens for this model, and that \
                             figure is used. This setting applies only to an endpoint that \
                             reports none."
                        ),
                        None => "Used when the endpoint does not report the model's context \
                                 window. Checked each time a project opens."
                            .to_string(),
                    })
                    .small()
                    .color(theme::TEXT_MUTED),
                );

                if let Some(error) = &self.error {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(error).color(theme::ERROR));
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Save").color(theme::TEXT_STRONG),
                                )
                                .fill(theme::ACCENT.gamma_multiply(0.55)),
                            )
                            .clicked()
                        {
                            action = SettingsAction::Save(form.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            action = SettingsAction::Cancel;
                        }
                    });
                });
            });

        if !open {
            action = SettingsAction::Cancel;
        }
        if action == SettingsAction::Cancel {
            self.close();
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_converts_to_limits_with_sane_floors() {
        let form = SettingsForm {
            reasoning_effort: String::new(),
            base_url: String::new(),
            model: String::new(),
            accepts_images: false,
            allow_insecure_http: false,
            credential: String::new(),
            has_stored_credential: false,
            credential_from_environment: false,
            wall_clock_seconds: 0,
            memory_megabytes: 1,
            context_window_tokens: 1,
            detected_context_window: None,
        };
        let limits = form.limits();
        assert_eq!(limits.wall_clock, Duration::from_secs(1));
        assert_eq!(limits.memory_bytes, 64 * 1024 * 1024);
    }
}
