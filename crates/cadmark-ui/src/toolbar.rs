// Toolbar — design-step navigation, refresh, view framing, and export.

use cadmark_core::export::ExportFormat;
use cadmark_core::version::VersionHistory;

/// Action taken by the toolbar controls.
pub enum ToolbarAction {
    None,
    Undo,
    Redo,
    /// User selected a specific microversion from the dropdown.
    JumpToVersion(usize),
    /// Re-execute the current script and reload the model.
    Refresh,
    /// Frame the whole model in the viewport.
    FitView,
    /// Write the current model to a file in the given format.
    Export(ExportFormat),
}

/// What the toolbar may offer right now.
#[derive(Debug, Clone, Copy)]
pub struct ToolbarState {
    /// Undo, redo and refresh are held while the worker is busy so a
    /// checkout cannot race an edit being written.
    pub controls_enabled: bool,
    /// Fit-view and export need a loaded model.
    pub has_model: bool,
}

/// Render the toolbar.
pub fn show_toolbar(
    ui: &mut egui::Ui,
    history: &VersionHistory,
    state: ToolbarState,
) -> ToolbarAction {
    let mut action = ToolbarAction::None;

    ui.horizontal(|ui| {
        // Undo button with dropdown.
        ui.add_enabled_ui(state.controls_enabled && history.can_undo(), |ui| {
            let undo_response = ui.button("Undo");

            if undo_response.clicked() {
                action = ToolbarAction::Undo;
            }

            // Dropdown on right-click or long-press showing recent versions.
            undo_response.context_menu(|ui| {
                ui.label(egui::RichText::new("Recent versions").strong().small());
                ui.separator();
                for (i, version) in history.recent(10).iter().enumerate() {
                    let label = format!(
                        "{} — {}",
                        version.summary,
                        truncate(&version.trigger_message, 40),
                    );
                    if ui.button(&label).clicked() {
                        action = ToolbarAction::JumpToVersion(i);
                        ui.close_menu();
                    }
                }
            });
        });

        // Redo button.
        if ui
            .add_enabled(
                state.controls_enabled && history.can_redo(),
                egui::Button::new("Redo"),
            )
            .clicked()
        {
            action = ToolbarAction::Redo;
        }

        ui.separator();

        // Refresh button — re-execute the script from disk.
        if ui
            .add_enabled(
                state.controls_enabled,
                egui::Button::new("\u{21BB} Refresh"),
            )
            .on_hover_text("Re-execute part.py and reload the model")
            .clicked()
        {
            action = ToolbarAction::Refresh;
        }

        if ui
            .add_enabled(state.has_model, egui::Button::new("Fit view"))
            .on_hover_text("Frame the whole model")
            .clicked()
        {
            action = ToolbarAction::FitView;
        }

        ui.separator();
        ui.label(egui::RichText::new("Export").small());
        for format in ExportFormat::ALL {
            if ui
                .add_enabled(state.has_model, egui::Button::new(format.label()))
                .on_hover_text(format!("Write part.{} next to part.py", format.extension()))
                .clicked()
            {
                action = ToolbarAction::Export(format);
            }
        }
    });

    action
}

/// Truncate a string to at most `max` characters, appending "..." if shortened.
/// Operates on char boundaries to avoid panicking on multi-byte UTF-8.
fn truncate(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max {
        s.to_string()
    } else {
        let end = s.char_indices().nth(max).map_or(s.len(), |(i, _)| i);
        format!("{}...", &s[..end])
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
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    #[test]
    fn truncate_multibyte_characters() {
        // Each emoji is 4 bytes — byte-level slicing would panic.
        let emoji = "\u{1F600}\u{1F601}\u{1F602}";
        let result = truncate(emoji, 2);
        assert_eq!(result, "\u{1F600}\u{1F601}...");
    }

    #[test]
    fn truncate_exact_length() {
        assert_eq!(truncate("abcde", 5), "abcde");
    }
}
