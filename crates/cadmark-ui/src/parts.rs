// Parts list — every part the executed script defines, in the order the
// script binds them, with the colour the viewport draws it in.
//
// A multi-part model needs a place that names its parts, says which one a
// click resolves against, and lets one be taken out of the way to see
// what is behind it. The list is that place: a row per part with its
// swatch, its name, a visibility toggle, and a warning when the kernel
// found it not to be a closed solid. Clicking a row selects the whole
// part, as a viewport click on it would.

use crate::theme;

/// One part as the list shows it.
pub struct PartRow<'a> {
    /// The part's ordinal within the execution, stable for one build.
    pub id: u32,
    /// The script binding that produced the part.
    pub name: &'a str,
    /// The colour the viewport draws the part in, display-encoded.
    pub colour: egui::Color32,
    /// Whether the viewport is drawing it.
    pub visible: bool,
    /// Whether the kernel found it a closed, valid solid.
    pub printable: bool,
    /// One line of measurements: volume, size, faces.
    pub summary: &'a str,
}

/// What the list shows.
pub struct PartsView<'a> {
    pub parts: &'a [PartRow<'a>],
    /// The part whose numbering the current selection uses.
    pub active: Option<u32>,
    /// Whether the script on screen has run at all.
    pub has_script: bool,
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartsAction {
    None,
    /// Select this whole part.
    Select(u32),
    /// Draw this part, or stop drawing it.
    SetVisible(u32, bool),
}

/// Side of the colour swatch, in points.
const SWATCH_SIZE: f32 = 12.0;

/// Render the list. Returns what the user asked for, if anything.
pub fn show_parts(ui: &mut egui::Ui, view: PartsView<'_>) -> PartsAction {
    let mut action = PartsAction::None;

    if view.parts.is_empty() {
        let message = if view.has_script {
            "The script produced no completed part. Parts appear here once \
             a BuildPart finishes or a solid is bound at the top level."
        } else {
            "Parts appear here once the model has been built."
        };
        ui.label(
            egui::RichText::new(message)
                .size(theme::SMALL_SIZE)
                .color(theme::TEXT_MUTED),
        );
        return action;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for part in view.parts {
                if let Some(asked) = show_row(ui, part, view.active == Some(part.id)) {
                    action = asked;
                }
            }
        });
    action
}

/// One row; `Some(action)` when the user did something to it.
fn show_row(ui: &mut egui::Ui, part: &PartRow<'_>, active: bool) -> Option<PartsAction> {
    let mut action = None;
    let fill = if active {
        theme::RAISED
    } else {
        egui::Color32::TRANSPARENT
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // The swatch is the same colour the viewport shades the part
                // in, so the list and the model read as one.
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(SWATCH_SIZE, SWATCH_SIZE),
                    egui::Sense::hover(),
                );
                let swatch = if part.visible {
                    part.colour
                } else {
                    part.colour.gamma_multiply(0.35)
                };
                ui.painter().rect_filled(rect, 3.0, swatch);
                ui.painter()
                    .rect_stroke(rect, 3.0, (1.0, theme::BORDER), egui::StrokeKind::Inside);

                let name_colour = match (active, part.visible) {
                    (true, _) => theme::TEXT_STRONG,
                    (false, true) => theme::TEXT,
                    (false, false) => theme::TEXT_MUTED,
                };
                let name = ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(part.name)
                                .monospace()
                                .color(name_colour),
                        )
                        .sense(egui::Sense::click())
                        .truncate(),
                    )
                    .on_hover_text(if part.summary.is_empty() {
                        "Click to select the whole part".to_string()
                    } else {
                        format!("{}\nClick to select the whole part", part.summary)
                    });
                if name.clicked() {
                    action = Some(PartsAction::Select(part.id));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut visible = part.visible;
                    let toggle = ui
                        .add(egui::Checkbox::without_text(&mut visible))
                        .on_hover_text(if part.visible {
                            "Shown in the viewport; untick to hide it"
                        } else {
                            "Hidden from the viewport; tick to show it"
                        });
                    if toggle.changed() {
                        action = Some(PartsAction::SetVisible(part.id, visible));
                    }
                    if !part.printable {
                        ui.label(
                            egui::RichText::new("\u{26A0}")
                                .color(theme::WARNING)
                                .size(theme::SMALL_SIZE),
                        )
                        .on_hover_text("Not a closed, valid solid: export will warn");
                    }
                });
            });
        });
    action
}
