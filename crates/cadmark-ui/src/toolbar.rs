// Toolbar — project, design-step navigation, view controls, code and export.
//
// One row across the top of the window. Reading left to right: which
// project is open, where you are in its history, what you can do to the
// view, and how to get the model out.

use std::path::Path;

use cadmark_core::export::ExportFormat;
use cadmark_core::version::{Microversion, VersionHistory};

/// The standard views the View menu offers, mirrored from the camera so
/// the toolbar names no renderer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Isometric,
}

impl StandardView {
    pub const ALL: [StandardView; 7] = [
        Self::Front,
        Self::Back,
        Self::Left,
        Self::Right,
        Self::Top,
        Self::Bottom,
        Self::Isometric,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Front => "Front",
            Self::Back => "Back",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Isometric => "Isometric",
        }
    }
}

use crate::theme;

/// Action taken by the toolbar controls.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolbarAction {
    None,
    Undo,
    Redo,
    /// User selected a specific design step from the history menu.
    JumpToVersion(usize),
    /// Open the version-naming dialog.
    NameVersion,
    /// Choose another project folder.
    OpenProject,
    /// Start a part with no name yet in the open project.
    NewPart,
    /// Open another part of the open project, by file name.
    OpenPart(String),
    /// Archive the current chat and begin a blank conversation.
    NewConversation,
    /// Open one of the recently used project folders.
    OpenRecent(std::path::PathBuf),
    /// Show the project folder in the system file manager.
    RevealProject,
    /// Open the script in the system's default editor.
    OpenScriptInEditor,
    /// Re-execute the current script and reload the model.
    Refresh,
    /// Frame the whole model in the viewport.
    FitView,
    /// The next viewport click selects a completed part, not a face or edge.
    PickPart,
    /// Show or hide the code panel.
    ToggleCode,
    /// Write the current model to a file in the given format.
    Export(ExportFormat),
    ExportPart(u32, ExportFormat),
    ExportAll(ExportFormat),
    /// Open the settings dialog.
    OpenSettings,
    /// Switch between perspective and orthographic projection.
    ToggleProjection,
    /// Snap the camera to a standard view.
    StandardView(StandardView),
}

/// One part the user can switch to, as the toolbar shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartOption {
    /// The script file inside the project folder.
    pub file_name: String,
    /// How the part is named for the user.
    pub display: String,
}

/// What the toolbar may offer right now.
#[derive(Debug, Clone)]
pub struct ToolbarState<'a> {
    /// The open project folder.
    pub project_dir: &'a Path,
    /// Name of the open part's script file inside the project.
    pub script_filename: &'a str,
    /// How the open part is named for the user; "Untitled" until its
    /// first save.
    pub part_name: &'a str,
    /// Every part the folder holds, in the order they are offered.
    pub parts: &'a [PartOption],
    /// Whether the script exists on disk yet.
    pub has_script: bool,
    /// Recently opened project folders, most recent first, excluding the
    /// current one.
    pub recent_projects: &'a [std::path::PathBuf],
    /// Undo, redo, refresh and project switching are held while the worker
    /// is busy so a checkout cannot race an edit being written.
    pub controls_enabled: bool,
    /// Fit-view and export need a loaded model.
    pub has_model: bool,
    /// The parts the executed script defines, as picking ID, script binding
    /// name, and whether the part is a closed valid solid.
    pub model_parts: &'a [(u32, String, bool)],
    /// Whether the code panel is showing.
    pub code_visible: bool,
    /// Whether the viewport is orthographic.
    pub orthographic: bool,
    /// A non-blocking explanation shown before an unavailable export.
    pub export_warning: Option<&'a str>,
    /// The AI model in use, or `None` when AI is unavailable.
    pub ai_model: Option<&'a str>,
}

/// The name a project folder is shown under: its final path component.
pub fn project_display_name(project_dir: &Path) -> String {
    project_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| project_dir.display().to_string())
}

/// One line describing a design step for menus and tooltips.
fn version_line(version: &Microversion) -> String {
    match &version.snapshot {
        Some(snapshot) => snapshot.name.clone(),
        None => version.summary.clone(),
    }
}

/// Render the toolbar.
pub fn show_toolbar(
    ui: &mut egui::Ui,
    history: &VersionHistory,
    state: ToolbarState<'_>,
) -> ToolbarAction {
    let mut action = ToolbarAction::None;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        // ── Project ────────────────────────────────────────────────
        let project_name = project_display_name(state.project_dir);
        let project_title =
            egui::RichText::new(format!("\u{1F4C1} {project_name}")).color(theme::TEXT_STRONG);
        ui.add_enabled_ui(state.controls_enabled, |ui| {
            ui.menu_button(project_title, |ui| {
                ui.set_min_width(260.0);
                ui.label(
                    egui::RichText::new(state.project_dir.display().to_string())
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "Every accepted edit is saved to {} and recorded as a design step.",
                        state.script_filename
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                ui.separator();
                if ui
                    .add(egui::Button::new("Open project folder\u{2026}").shortcut_text("Ctrl+O"))
                    .clicked()
                {
                    action = ToolbarAction::OpenProject;
                    ui.close_menu();
                }
                if ui
                    .button("New conversation")
                    .on_hover_text(
                        "Archive this chat and start a blank one; the script is unchanged",
                    )
                    .clicked()
                {
                    action = ToolbarAction::NewConversation;
                    ui.close_menu();
                }
                if !state.recent_projects.is_empty() {
                    ui.menu_button("Open recent", |ui| {
                        ui.set_min_width(240.0);
                        for path in state.recent_projects {
                            let name = project_display_name(path);
                            if ui
                                .button(name)
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                action = ToolbarAction::OpenRecent(path.clone());
                                ui.close_menu();
                            }
                        }
                    });
                }
                ui.separator();
                if ui
                    .add_enabled(
                        state.has_script,
                        egui::Button::new(format!("Open {} in editor", state.script_filename)),
                    )
                    .clicked()
                {
                    action = ToolbarAction::OpenScriptInEditor;
                    ui.close_menu();
                }
                if ui.button("Show folder in file manager").clicked() {
                    action = ToolbarAction::RevealProject;
                    ui.close_menu();
                }
            });

            // ── Part ───────────────────────────────────────────────
            // A folder holds any number of parts; this is how the user
            // moves between them without leaving the app.
            let part_title = egui::RichText::new(format!("{} \u{25BE}", state.part_name))
                .color(theme::TEXT_STRONG);
            ui.menu_button(part_title, |ui| {
                ui.set_min_width(220.0);
                ui.label(
                    egui::RichText::new("Parts in this folder")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                for part in state.parts {
                    let open = part.file_name == state.script_filename;
                    if ui.selectable_label(open, &part.display).clicked() && !open {
                        action = ToolbarAction::OpenPart(part.file_name.clone());
                        ui.close_menu();
                    }
                }
                if state.parts.is_empty() {
                    ui.label(
                        egui::RichText::new("No parts yet")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
                ui.separator();
                if ui
                    .button("New part")
                    .on_hover_text("Start another part in this folder; it is named when you save")
                    .clicked()
                {
                    action = ToolbarAction::NewPart;
                    ui.close_menu();
                }
            });
        });

        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);

        // ── History ────────────────────────────────────────────────
        let undo_hint = history
            .peek_undo()
            .map(|version| format!("Undo: {}", version_line(version)))
            .unwrap_or_else(|| "Nothing to undo".to_string());
        if ui
            .add_enabled(
                state.controls_enabled && history.can_undo(),
                egui::Button::new("\u{21B6}"),
            )
            .on_hover_text(format!("{undo_hint}\nCtrl+Z"))
            .on_disabled_hover_text(&undo_hint)
            .clicked()
        {
            action = ToolbarAction::Undo;
        }

        let redo_hint = history
            .peek_redo()
            .map(|version| format!("Redo: {}", version_line(version)))
            .unwrap_or_else(|| "Nothing to redo".to_string());
        if ui
            .add_enabled(
                state.controls_enabled && history.can_redo(),
                egui::Button::new("\u{21B7}"),
            )
            .on_hover_text(format!("{redo_hint}\nCtrl+Shift+Z"))
            .on_disabled_hover_text(&redo_hint)
            .clicked()
        {
            action = ToolbarAction::Redo;
        }

        let step_count = history.len();
        let history_title = match history.current() {
            Some(current) => format!(
                "{}/{} · {} \u{25BE}",
                step_count - history.current_index(),
                step_count,
                truncate(&version_line(current), 28)
            ),
            None => "History \u{25BE}".to_string(),
        };
        ui.add_enabled_ui(state.controls_enabled, |ui| {
            ui.menu_button(history_title, |ui| {
                ui.set_min_width(320.0);
                if ui
                    .add_enabled(
                        state.has_script,
                        egui::Button::new("Name this version\u{2026}").shortcut_text("Ctrl+S"),
                    )
                    .on_hover_text("Mark the current model as a named version you can return to")
                    .clicked()
                {
                    action = ToolbarAction::NameVersion;
                    ui.close_menu();
                }
                ui.separator();
                if step_count == 0 {
                    ui.label(
                        egui::RichText::new("No design steps yet")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!(
                            "{step_count} design step{}, newest first",
                            if step_count == 1 { "" } else { "s" }
                        ))
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for (index, version) in history.recent(50).iter().enumerate() {
                            let is_current = index == history.current_index();
                            let marker = if is_current { "\u{25CF}" } else { "\u{2007}" };
                            let mut title = egui::RichText::new(format!(
                                "{marker} {}",
                                truncate(&version_line(version), 72)
                            ));
                            if version.snapshot.is_some() {
                                title = title.strong().color(theme::WARNING);
                            }
                            if is_current {
                                title = title.color(theme::ACCENT);
                            }
                            let response = ui
                                .add(egui::Button::new(title).frame(false).selected(is_current))
                                .on_hover_text(version_tooltip(version));
                            if response.clicked() && !is_current {
                                action = ToolbarAction::JumpToVersion(index);
                                ui.close_menu();
                            }
                        }
                    });
            });
        });

        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);

        // ── View ───────────────────────────────────────────────────
        if ui
            .add_enabled(
                state.controls_enabled && state.has_script,
                egui::Button::new("\u{21BB} Rebuild"),
            )
            .on_hover_text(format!(
                "Run {} again and reload the model\nF5",
                state.script_filename
            ))
            .clicked()
        {
            action = ToolbarAction::Refresh;
        }

        if ui
            .add_enabled(state.has_model, egui::Button::new("\u{22A1} Fit"))
            .on_hover_text("Frame the whole model\nF")
            .clicked()
        {
            action = ToolbarAction::FitView;
        }

        if ui
            .add_enabled(
                !state.model_parts.is_empty(),
                egui::Button::new("Pick part"),
            )
            .on_hover_text("Select a whole part with the next viewport click")
            .clicked()
        {
            action = ToolbarAction::PickPart;
        }

        ui.menu_button("View \u{25BE}", |ui| {
            ui.set_min_width(180.0);
            let projection = if state.orthographic {
                "Perspective"
            } else {
                "Orthographic"
            };
            if ui
                .add(egui::Button::new(format!("Switch to {projection}")).shortcut_text("P"))
                .clicked()
            {
                action = ToolbarAction::ToggleProjection;
                ui.close_menu();
            }
            ui.separator();
            for view in StandardView::ALL {
                if ui.button(view.label()).clicked() {
                    action = ToolbarAction::StandardView(view);
                    ui.close_menu();
                }
            }
        });

        if ui
            .add(egui::Button::new("Code").selected(state.code_visible))
            .on_hover_text(format!(
                "Show the {} that builds this model\nCtrl+E",
                state.script_filename
            ))
            .clicked()
        {
            action = ToolbarAction::ToggleCode;
        }

        ui.add_enabled_ui(state.has_model, |ui| {
            ui.menu_button("Export \u{25BE}", |ui| {
                ui.set_min_width(220.0);
                if let Some(warning) = state.export_warning {
                    ui.label(egui::RichText::new(warning).small().color(theme::WARNING));
                    ui.separator();
                }
                for format in ExportFormat::ALL {
                    if ui
                        .button(format!("{} (part.{})", format.label(), format.extension()))
                        .on_hover_text(format!(
                            "Write part.{} next to {}",
                            format.extension(),
                            state.script_filename
                        ))
                        .clicked()
                    {
                        action = ToolbarAction::Export(format);
                        ui.close_menu();
                    }
                }
                if state.model_parts.len() > 1 {
                    ui.separator();
                    for format in ExportFormat::ALL {
                        if ui
                            .button(format!("Export all parts as {}", format.label()))
                            .clicked()
                        {
                            action = ToolbarAction::ExportAll(format);
                            ui.close_menu();
                        }
                    }
                    for (id, name, printable) in state.model_parts {
                        ui.menu_button(
                            format!("{name}{}", if *printable { "" } else { " (warning)" }),
                            |ui| {
                                for format in ExportFormat::ALL {
                                    if ui.button(format!("Export as {}", format.label())).clicked()
                                    {
                                        action = ToolbarAction::ExportPart(*id, format);
                                        ui.close_menu();
                                    }
                                }
                            },
                        );
                    }
                }
            });
        });

        // ── Settings and the AI badge, right-aligned ────────────────
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("\u{2699}")
                .on_hover_text("Settings: AI provider and script limits")
                .clicked()
            {
                action = ToolbarAction::OpenSettings;
            }
            match state.ai_model {
                Some(model) => {
                    theme::chip(ui, model, theme::AI).on_hover_text("The AI model in use");
                }
                None => {
                    if theme::chip(ui, "AI off", theme::TEXT_MUTED)
                        .on_hover_text(
                            "No AI provider is configured. The model still loads and rebuilds; \
                             open Settings to add one.",
                        )
                        .interact(egui::Sense::click())
                        .clicked()
                    {
                        action = ToolbarAction::OpenSettings;
                    }
                }
            }
        });
    });

    action
}

fn version_tooltip(version: &Microversion) -> String {
    let mut lines = vec![version.summary.clone()];
    if !version.trigger_message.is_empty() {
        lines.push(format!(
            "\u{201C}{}\u{201D}",
            truncate(&version.trigger_message, 120)
        ));
    }
    lines.push(
        version
            .timestamp
            .with_timezone(&chrono::Local)
            .format("%-d %b %Y, %H:%M")
            .to_string(),
    );
    lines.join("\n")
}

/// Truncate a string to at most `max` characters, appending an ellipsis if
/// shortened. Operates on char boundaries to avoid panicking on multi-byte
/// UTF-8.
pub fn truncate(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max {
        s.to_string()
    } else {
        let end = s.char_indices().nth(max).map_or(s.len(), |(i, _)| i);
        format!("{}\u{2026}", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_ascii() {
        assert_eq!(truncate("hello world", 5), "hello\u{2026}");
    }

    #[test]
    fn truncate_multibyte_characters() {
        // Each emoji is 4 bytes — byte-level slicing would panic.
        let emoji = "\u{1F600}\u{1F601}\u{1F602}";
        let result = truncate(emoji, 2);
        assert_eq!(result, "\u{1F600}\u{1F601}\u{2026}");
    }

    #[test]
    fn truncate_exact_length() {
        assert_eq!(truncate("abcde", 5), "abcde");
    }

    #[test]
    fn project_name_is_the_folder_name() {
        assert_eq!(
            project_display_name(Path::new("/home/someone/parts/bracket")),
            "bracket"
        );
        assert_eq!(project_display_name(Path::new("/")), "/");
    }

    #[test]
    fn named_versions_show_their_name_not_the_summary() {
        let mut version = Microversion {
            commit_hash: "abc1234".into(),
            summary: "Snapshot: Ready for print".into(),
            trigger_message: String::new(),
            timestamp: chrono::Utc::now(),
            snapshot: None,
        };
        assert_eq!(version_line(&version), "Snapshot: Ready for print");
        version.snapshot = Some(cadmark_core::version::SnapshotInfo {
            name: "Ready for print".into(),
        });
        assert_eq!(version_line(&version), "Ready for print");
    }
}
