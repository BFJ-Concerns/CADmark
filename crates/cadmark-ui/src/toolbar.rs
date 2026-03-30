// Toolbar — undo/redo controls with microversion dropdown.

use cadmark_core::version::VersionHistory;

/// Action taken by the toolbar controls.
pub enum ToolbarAction {
    None,
    Undo,
    Redo,
    /// User selected a specific microversion from the dropdown.
    JumpToVersion(usize),
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
    });

    action
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.min(s.len())])
    }
}
