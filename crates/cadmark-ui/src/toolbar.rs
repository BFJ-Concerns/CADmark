// Toolbar — project, design-step navigation, view controls, code and export.
//
// One row across the top of the window. Reading left to right: which
// project is open, where you are in its history, what you can do to the
// view, and how to get the model out.

use std::path::Path;

use cadmark_core::export::ExportFormat;
use cadmark_core::version::{Microversion, VersionHistory};

/// The kinds of element a click may land on, mirrored from the renderer
/// so the toolbar names no renderer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    Face,
    Edge,
    Vertex,
}

impl SelectionKind {
    pub const ALL: [SelectionKind; 3] = [Self::Face, Self::Edge, Self::Vertex];

    pub fn label(self) -> &'static str {
        match self {
            Self::Face => "Faces",
            Self::Edge => "Edges",
            Self::Vertex => "Vertices",
        }
    }
}

/// Which kinds are currently clickable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionKinds {
    pub faces: bool,
    pub edges: bool,
    pub vertices: bool,
}

impl SelectionKinds {
    pub fn enabled(self, kind: SelectionKind) -> bool {
        match kind {
            SelectionKind::Face => self.faces,
            SelectionKind::Edge => self.edges,
            SelectionKind::Vertex => self.vertices,
        }
    }

    /// A short summary for the control's own label: nothing when every
    /// kind is clickable, otherwise what is left.
    pub fn summary(self) -> Option<String> {
        let enabled: Vec<&str> = SelectionKind::ALL
            .iter()
            .filter(|kind| self.enabled(**kind))
            .map(|kind| kind.label())
            .collect();
        if enabled.len() == SelectionKind::ALL.len() {
            None
        } else if enabled.is_empty() {
            Some("nothing".to_string())
        } else {
            Some(enabled.join(", ").to_lowercase())
        }
    }
}

use crate::theme;

/// The axis a section plane cuts along, mirrored from the renderer so the
/// toolbar names no renderer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionAxis {
    X,
    Y,
    Z,
}

impl SectionAxis {
    pub const ALL: [SectionAxis; 3] = [Self::X, Self::Y, Self::Z];

    pub fn label(self) -> &'static str {
        match self {
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
        }
    }
}

/// The section plane's state, as the toolbar needs to draw it.
#[derive(Debug, Clone, Copy)]
pub struct SectionState {
    pub enabled: bool,
    pub axis: SectionAxis,
    pub offset: f32,
    pub flipped: bool,
    /// The model's extent along the current axis, which bounds the slider.
    /// A plane the user cannot drag past the part is a plane that always
    /// shows something.
    pub range: (f32, f32),
}

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
    /// Open one of the recently used project folders.
    OpenRecent(std::path::PathBuf),
    /// Show the project folder in the system file manager.
    RevealProject,
    /// Open the script in the system's default editor.
    OpenScriptInEditor,
    /// Re-execute the current script and reload the model.
    Refresh,
    /// Show or hide the code panel.
    ToggleCode,
    /// Write the current model to a file in the given format.
    Export(ExportFormat),
    ExportPart(u32, ExportFormat),
    ExportAll(ExportFormat),
    /// Open the settings dialog.
    OpenSettings,
    /// Turn one kind of element on or off for clicking.
    ToggleSelectionKind(SelectionKind),
    /// Turn the section plane on or off.
    ToggleSection,
    /// Cut the section along a different axis.
    SetSectionAxis(SectionAxis),
    /// Move the section plane to a position along its axis.
    SetSectionOffset(f32),
    /// Reverse which half of the model the section keeps.
    FlipSection,
    /// Switch the model between solid and see-through.
    ToggleTransparency,
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
    /// Export needs a loaded model.
    pub has_model: bool,
    /// The formats the model on screen can be written to: solid formats
    /// for a solid, drawings and STEP for a sketch.
    pub export_formats: &'a [ExportFormat],
    /// The name of the part the top-level export writes, when the model
    /// has several and one is selected.
    pub export_target: Option<&'a str>,
    /// The parts the executed script defines, as part ordinal, script
    /// binding name, and whether the part is a closed valid solid.
    pub model_parts: &'a [(u32, String, bool)],
    /// Whether the code panel is showing.
    pub code_visible: bool,
    /// A non-blocking explanation shown before an unavailable export.
    pub export_warning: Option<&'a str>,
    /// Which kinds of element a click may land on.
    pub selection_kinds: SelectionKinds,
    /// The AI model in use, or `None` when AI is unavailable.
    pub ai_model: Option<&'a str>,
    /// The section plane's current state.
    pub section: SectionState,
    /// Whether the model is drawn see-through.
    pub transparent: bool,
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

        let selection_label = match state.selection_kinds.summary() {
            Some(summary) => format!("Select: {summary} \u{25BE}"),
            None => "Select \u{25BE}".to_string(),
        };
        ui.menu_button(selection_label, |ui| {
            ui.set_min_width(180.0);
            ui.label(
                egui::RichText::new("What a click in the viewport can land on")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            for kind in SelectionKind::ALL {
                let mut enabled = state.selection_kinds.enabled(kind);
                if ui.checkbox(&mut enabled, kind.label()).changed() {
                    action = ToolbarAction::ToggleSelectionKind(kind);
                }
            }
            ui.separator();
            ui.label(
                egui::RichText::new("Alt+click selects a whole part")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        })
        .response
        .on_hover_text("Turn a kind off to click past it to what is behind");

        // ── Seeing inside the part ─────────────────────────────────
        // Both controls sit in the row itself rather than behind a menu or
        // a dialog: they are adjustments the user makes while looking at
        // the model, and neither takes the viewport or the chat away.
        ui.add_enabled_ui(state.has_model, |ui| {
            if ui
                .add(egui::Button::new("\u{2702} Section").selected(state.section.enabled))
                .on_hover_text("Cut away half the model to see inside it")
                .clicked()
            {
                action = ToolbarAction::ToggleSection;
            }

            if state.section.enabled {
                for axis in SectionAxis::ALL {
                    if ui
                        .add(
                            egui::Button::new(axis.label())
                                .selected(axis == state.section.axis)
                                .min_size(egui::vec2(20.0, 0.0)),
                        )
                        .on_hover_text(format!("Cut along {}", axis.label()))
                        .clicked()
                    {
                        action = ToolbarAction::SetSectionAxis(axis);
                    }
                }

                let (low, high) = state.section.range;
                let mut offset = state.section.offset;
                if ui
                    .add(
                        egui::Slider::new(&mut offset, low..=high)
                            .show_value(false)
                            .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.4 }),
                    )
                    .on_hover_text("Move the section plane along its axis")
                    .changed()
                {
                    action = ToolbarAction::SetSectionOffset(offset);
                }

                if ui
                    .add(egui::Button::new("\u{21C4}").selected(state.section.flipped))
                    .on_hover_text("Keep the other half instead")
                    .clicked()
                {
                    action = ToolbarAction::FlipSection;
                }
            }

            if ui
                .add(egui::Button::new("\u{25CE} Ghost").selected(state.transparent))
                .on_hover_text("Make the model see-through")
                .clicked()
            {
                action = ToolbarAction::ToggleTransparency;
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
                if let Some(target) = state.export_target {
                    ui.label(
                        egui::RichText::new(format!("Selected part: {target}"))
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
                let stem = state.part_name;
                for &format in state.export_formats {
                    let file_name = match state.export_target {
                        Some(target) => format!("{stem}-{target}.{}", format.extension()),
                        None => format!("{stem}.{}", format.extension()),
                    };
                    if ui
                        .button(format!("{} ({file_name})", format.label()))
                        .on_hover_text(format!(
                            "Write {file_name} next to {}",
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
                    for &format in state.export_formats {
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
                                for &format in state.export_formats {
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
