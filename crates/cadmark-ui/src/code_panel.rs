// Code panel — the build123d script that produced the model, read-only.
//
// The script is the artifact the user owns, so it is always one click
// away: numbered lines, the line a spatial comment resolved to highlighted,
// and buttons to copy the source or open it in the system editor.

use crate::theme;

/// Actions the code panel can request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodePanelAction {
    None,
    /// Copy the whole script to the clipboard.
    Copy,
    /// Open the script in the system's default editor.
    OpenInEditor,
    /// Re-run the script from disk (after an external edit).
    Refresh,
}

/// What the code panel shows.
pub struct CodeView<'a> {
    pub script_filename: &'a str,
    /// The script as last executed, or `None` when there is no script yet.
    pub source: Option<&'a str>,
    /// Line to highlight, from the current selection's provenance.
    pub highlighted_line: Option<u32>,
    /// Whether the script on disk differs from the last executed source.
    pub modified_on_disk: bool,
    /// Whether refresh is currently permitted.
    pub controls_enabled: bool,
}

/// State kept between frames.
#[derive(Debug, Default)]
pub struct CodePanel {
    /// Highlighted line on the previous frame, so a change scrolls to it once.
    last_highlight: Option<u32>,
}

impl CodePanel {
    /// Render the panel. Returns the action the user requested.
    pub fn show(&mut self, ui: &mut egui::Ui, view: CodeView<'_>) -> CodePanelAction {
        let mut action = CodePanelAction::None;

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(view.script_filename)
                    .strong()
                    .color(theme::TEXT_STRONG)
                    .monospace(),
            );
            if view.modified_on_disk {
                theme::chip(ui, "changed on disk", theme::WARNING).on_hover_text(
                    "The file differs from the model on screen. Rebuild to load it.",
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(view.source.is_some(), egui::Button::new("Copy").small())
                    .on_hover_text("Copy the whole script")
                    .clicked()
                {
                    action = CodePanelAction::Copy;
                }
                if ui
                    .add_enabled(view.source.is_some(), egui::Button::new("Edit").small())
                    .on_hover_text("Open in your editor; press Rebuild afterwards")
                    .clicked()
                {
                    action = CodePanelAction::OpenInEditor;
                }
                if view.modified_on_disk
                    && ui
                        .add_enabled(view.controls_enabled, egui::Button::new("Rebuild").small())
                        .clicked()
                {
                    action = CodePanelAction::Refresh;
                }
            });
        });
        ui.add_space(2.0);

        let Some(source) = view.source else {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("No script yet. Describe a part to create one.")
                        .color(theme::TEXT_MUTED),
                );
            });
            return action;
        };

        let scroll_to_highlight = view.highlighted_line != self.last_highlight;
        self.last_highlight = view.highlighted_line;

        egui::Frame::new()
            .fill(theme::SUNKEN)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
            .corner_radius(egui::CornerRadius::same(theme::RADIUS))
            .inner_margin(egui::Margin::symmetric(0, 6))
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_salt("code_panel_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let line_count = source.lines().count().max(1);
                        let gutter_width = line_count.to_string().len();
                        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 3.0;
                        let full_width = ui.available_width().max(400.0);

                        for (index, line) in source.lines().enumerate() {
                            let number = index as u32 + 1;
                            let highlighted = view.highlighted_line == Some(number);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(full_width, row_height),
                                egui::Sense::hover(),
                            );
                            if highlighted {
                                ui.painter().rect_filled(
                                    rect,
                                    0.0,
                                    theme::ACCENT.gamma_multiply(0.18),
                                );
                                ui.painter().rect_filled(
                                    egui::Rect::from_min_size(
                                        rect.min,
                                        egui::vec2(3.0, rect.height()),
                                    ),
                                    0.0,
                                    theme::ACCENT,
                                );
                                if scroll_to_highlight {
                                    ui.scroll_to_rect(rect, Some(egui::Align::Center));
                                }
                            }
                            let gutter = format!("{number:>gutter_width$}");
                            let font = egui::TextStyle::Monospace.resolve(ui.style());
                            ui.painter().text(
                                egui::pos2(rect.left() + 10.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                gutter,
                                font.clone(),
                                if highlighted {
                                    theme::ACCENT
                                } else {
                                    theme::TEXT_MUTED
                                },
                            );
                            let gutter_px = gutter_width as f32 * font.size * 0.62 + 24.0;
                            ui.painter().text(
                                egui::pos2(rect.left() + gutter_px, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                line,
                                font,
                                if highlighted {
                                    theme::TEXT_STRONG
                                } else {
                                    theme::TEXT
                                },
                            );
                        }
                    });
            });

        action
    }
}
