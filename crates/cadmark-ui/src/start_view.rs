// Start view — what CADmark shows before a project is chosen.
//
// The application opens here and loads nothing: the user picks one of the
// folders they worked in recently, opens another, or creates a new one.
// Until they do, there is no project, no worker and no model.

use std::path::{Path, PathBuf};

use crate::theme;
use crate::toolbar::project_display_name;

/// What the start view may offer right now.
#[derive(Debug, Clone)]
pub struct StartViewState<'a> {
    /// Project folders the user worked in before, most recent first.
    pub recent_projects: &'a [PathBuf],
    /// Why the last attempt to open a folder failed, if one did.
    pub notice: Option<&'a str>,
    /// Held while a folder picker is already on screen.
    pub controls_enabled: bool,
}

/// What the user chose to do from the start view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartAction {
    None,
    /// Open a folder from the recent list.
    OpenRecent(PathBuf),
    /// Choose a folder with the system picker.
    OpenFolder,
    /// Choose a folder for a new project.
    CreateProject,
    /// Open the settings dialog before choosing anything.
    OpenSettings,
}

/// Render the start view into the central panel.
pub fn show_start_view(ui: &mut egui::Ui, state: StartViewState<'_>) -> StartAction {
    let mut action = StartAction::None;

    ui.vertical_centered(|ui| {
        ui.add_space(64.0);
        ui.label(
            egui::RichText::new("CADmark")
                .size(28.0)
                .color(theme::TEXT_STRONG),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Choose a project folder to work in.").color(theme::TEXT_MUTED),
        );
        ui.add_space(20.0);

        ui.allocate_ui_with_layout(
            egui::vec2(420.0, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.add_enabled_ui(state.controls_enabled, |ui| {
                    if ui
                        .add_sized(
                            [420.0, 32.0],
                            egui::Button::new(
                                egui::RichText::new("New project\u{2026}")
                                    .color(theme::TEXT_STRONG),
                            )
                            .fill(theme::ACCENT.gamma_multiply(0.55)),
                        )
                        .on_hover_text("Choose an empty folder to start a project in")
                        .clicked()
                    {
                        action = StartAction::CreateProject;
                    }
                    ui.add_space(6.0);
                    if ui
                        .add_sized(
                            [420.0, 32.0],
                            egui::Button::new("Open project folder\u{2026}"),
                        )
                        .clicked()
                    {
                        action = StartAction::OpenFolder;
                    }
                });

                if let Some(notice) = state.notice {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(notice).small().color(theme::WARNING));
                }

                ui.add_space(22.0);
                ui.label(
                    egui::RichText::new("Recent projects")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.add_space(6.0);
                if state.recent_projects.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "Nothing yet \u{2014} the projects you open appear here.",
                        )
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            ui.add_enabled_ui(state.controls_enabled, |ui| {
                                for path in state.recent_projects {
                                    if recent_row(ui, path).clicked() {
                                        action = StartAction::OpenRecent(path.clone());
                                    }
                                }
                            });
                        });
                }

                ui.add_space(20.0);
                if ui
                    .link(egui::RichText::new("Settings\u{2026}").small())
                    .clicked()
                {
                    action = StartAction::OpenSettings;
                }
            },
        );
    });

    action
}

/// One recent project: its folder name over its path.
fn recent_row(ui: &mut egui::Ui, path: &Path) -> egui::Response {
    let response = ui.add_sized(
        [420.0, 40.0],
        egui::Button::new(
            egui::RichText::new(format!("\u{1F4C1} {}", project_display_name(path)))
                .color(theme::TEXT),
        )
        .fill(theme::RAISED),
    );
    ui.label(
        egui::RichText::new(path.display().to_string())
            .small()
            .color(theme::TEXT_MUTED),
    );
    ui.add_space(6.0);
    response.on_hover_text(path.display().to_string())
}
