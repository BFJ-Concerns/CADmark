/// Consume plain Enter without treating Shift+Enter as submission.
/// egui's shortcut matching deliberately permits extra Shift/Alt modifiers.
pub(crate) fn consume_submit(ui: &mut egui::Ui, response: &egui::Response) -> bool {
    response.has_focus()
        && ui.input_mut(|input| {
            let mut submitted = false;
            input.events.retain(|event| {
                let submit = matches!(event, egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                modifiers,
                ..
            } if modifiers.is_none());
                submitted |= submit;
                !submit
            });
            submitted
        })
}
