// The left panel's tab strip: the same space beside the viewport holds
// the script's parameters or its parts, one at a time, and the strip is
// how the user chooses which. The script's file name sits at the right of
// the strip, since both tabs describe that one script.

use crate::theme;

/// Which of the left panel's views is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidePanelTab {
    #[default]
    Parameters,
    Parts,
}

/// What the strip shows beside the tabs.
pub struct SidePanelView<'a> {
    pub script_filename: &'a str,
    /// How many parts the executed script defines, shown on the Parts tab
    /// so a multi-part model announces itself.
    pub part_count: usize,
}

/// Render the strip, switching `tab` when the user picks the other one.
pub fn show_tabs(ui: &mut egui::Ui, tab: &mut SidePanelTab, view: SidePanelView<'_>) {
    ui.horizontal(|ui| {
        ui.selectable_value(tab, SidePanelTab::Parameters, "Parameters");
        let parts_label = if view.part_count > 0 {
            format!("Parts ({})", view.part_count)
        } else {
            "Parts".to_string()
        };
        ui.selectable_value(tab, SidePanelTab::Parts, parts_label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(view.script_filename)
                        .monospace()
                        .size(theme::SMALL_SIZE)
                        .color(theme::TEXT_MUTED),
                )
                .truncate(),
            );
        });
    });
}
