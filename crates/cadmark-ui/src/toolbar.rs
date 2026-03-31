// Toolbar — undo/redo controls with microversion dropdown.

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
}

/// Render the toolbar with undo/redo controls.
pub fn show_toolbar(
    ui: &mut egui::Ui,
    history: &VersionHistory,
) -> ToolbarAction {
    let mut action = ToolbarAction::None;

    ui.horizontal(|ui| {
        // Undo button with dropdown.
        ui.add_enabled_ui(history.can_undo(), |ui| {
            let undo_response = ui.button("Undo");

            if undo_response.clicked() {
                action = ToolbarAction::Undo;
            }

            // Dropdown on right-click or long-press showing recent versions.
            undo_response.context_menu(|ui| {
                ui.label(
                    egui::RichText::new("Recent versions")
                        .strong()
                        .small(),
                );
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
            .add_enabled(history.can_redo(), egui::Button::new("Redo"))
            .clicked()
        {
            action = ToolbarAction::Redo;
        }

        ui.separator();

        // Refresh button — re-execute the script from disk.
        if ui
            .button("\u{21BB} Refresh")
            .on_hover_text("Re-execute script and reload model")
            .clicked()
        {
            action = ToolbarAction::Refresh;
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
