// Status bar — what the worker is doing, else the last result, plus the
// model's measurements and the navigation hint.

use cadmark_core::geometry::ModelSummary;

use crate::theme;

/// A line for the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub is_error: bool,
}

impl Status {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Everything the status bar reads.
pub struct StatusView<'a> {
    /// What the worker is doing, if anything.
    pub activity: Option<&'a str>,
    /// The last outcome to report.
    pub status: Option<&'a Status>,
    /// Measurements of the model on screen.
    pub summary: Option<&'a ModelSummary>,
    /// What the user has selected, as a short label.
    pub selection: Option<String>,
}

/// Render the status bar contents.
pub fn show_status_bar(ui: &mut egui::Ui, view: StatusView<'_>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if let Some(activity) = view.activity {
            ui.add(egui::Spinner::new().size(12.0).color(theme::ACCENT));
            ui.label(egui::RichText::new(activity).color(theme::TEXT));
        } else if let Some(status) = view.status {
            let (dot, colour) = if status.is_error {
                ("\u{25CF}", theme::ERROR)
            } else {
                ("\u{25CF}", theme::SUCCESS)
            };
            ui.label(egui::RichText::new(dot).color(colour).small());
            ui.add(
                egui::Label::new(egui::RichText::new(&status.text).color(if status.is_error {
                    theme::ERROR
                } else {
                    theme::TEXT
                }))
                .truncate(),
            )
            .on_hover_text(&status.text);
        } else {
            ui.label(egui::RichText::new("Ready").color(theme::TEXT_MUTED));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(
                    "Right-drag orbit · Middle-drag pan · Scroll zoom · Click to comment",
                )
                .small()
                .color(theme::TEXT_MUTED),
            );
            if let Some(summary) = view.summary {
                ui.separator();
                ui.label(
                    egui::RichText::new(summary.describe())
                        .small()
                        .color(theme::TEXT_MUTED),
                )
                .on_hover_text("Faces, volume and bounding box of the model on screen");
            }
            if let Some(selection) = view.selection {
                ui.separator();
                theme::chip(ui, &selection, theme::SPATIAL);
            }
        });
    });
}
