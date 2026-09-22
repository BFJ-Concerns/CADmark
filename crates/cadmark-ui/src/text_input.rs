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

/// Whether the user pressed the paste shortcut with something other than
/// text on the clipboard. The windowing layer turns a text paste into a
/// `Paste` event and swallows the key press; when the clipboard holds no
/// text — an image, say — it swallows the press and emits nothing, so the
/// only trace of the shortcut is the key's release with the modifier still
/// held. A text paste's release looks the same, so the caller must treat a
/// true result as "worth reading the clipboard", not as proof of an image.
pub(crate) fn consume_image_paste(ui: &mut egui::Ui, response: &egui::Response) -> bool {
    response.has_focus()
        && ui.input(|input| {
            let pasted_text = input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Paste(_)));
            !pasted_text
                && input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::V,
                            pressed: false,
                            modifiers,
                            ..
                        } if modifiers.command
                    )
                })
        })
}
