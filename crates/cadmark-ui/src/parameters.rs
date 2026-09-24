// Parameters panel — the script's named numbers, editable in place.
//
// Adjusting a dimension is the one change the user makes without asking
// the AI (C16), so the panel sits beside the viewport rather than behind
// a mode, as one tab of the left panel: a row per module-level numeric
// name, a drag-or-type field for the ones bound to a literal, and the
// expression itself for the ones derived from other parameters.
// Committing a value is what the application turns into a rewritten
// script, a rebuild and a design step.

use crate::theme;

/// One row of the panel: a name from the script and how it is bound.
pub struct ParameterRow<'a> {
    pub name: &'a str,
    /// The value to edit, when the name is bound to a literal.
    pub value: Option<f64>,
    /// What is shown when there is nothing to edit — the expression.
    pub expression: &'a str,
    /// One-based source line, for the tooltip.
    pub line: u32,
}

/// What the panel shows.
pub struct ParametersView<'a> {
    pub parameters: &'a [ParameterRow<'a>],
    /// Whether the script on screen is one we could have parameters for.
    pub has_script: bool,
    /// Whether editing is permitted (nothing else in flight).
    pub controls_enabled: bool,
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum ParametersAction {
    None,
    /// Set this parameter to this value in the script.
    Commit {
        name: String,
        value: f64,
    },
}

/// State kept between frames: the value being dragged or typed, which is
/// not the script's value until the user lets go.
#[derive(Debug, Default)]
pub struct ParametersPanel {
    draft: Option<(String, f64)>,
}

impl ParametersPanel {
    /// Render the panel. Returns the edit the user committed, if any.
    pub fn show(&mut self, ui: &mut egui::Ui, view: ParametersView<'_>) -> ParametersAction {
        let mut action = ParametersAction::None;

        if view.parameters.is_empty() {
            let message = if view.has_script {
                "No named numbers at the top of the script yet. Ask for one \
                 — \"make the width a parameter\" — and it will appear here."
            } else {
                "Parameters appear here once the model has been built."
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
                for row in view.parameters {
                    let committed = self.show_row(ui, row, view.controls_enabled);
                    if let Some(value) = committed {
                        action = ParametersAction::Commit {
                            name: row.name.to_string(),
                            value,
                        };
                    }
                }
            });
        action
    }

    /// One row; `Some(value)` when the user finished an edit.
    fn show_row(
        &mut self,
        ui: &mut egui::Ui,
        row: &ParameterRow<'_>,
        enabled: bool,
    ) -> Option<f64> {
        let mut committed = None;
        ui.horizontal(|ui| {
            // The row never grows past its column: a name that does not
            // fit is cut short (egui shows the whole of a truncated label
            // on hover), and the value keeps a fixed share at the right.
            let name_width = (ui.available_width() - VALUE_WIDTH).max(0.0);
            ui.scope(|ui| {
                ui.set_max_width(name_width);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(row.name)
                            .monospace()
                            .color(theme::TEXT)
                            .size(theme::CODE_SIZE),
                    )
                    .truncate(),
                )
                .on_hover_text(format!("line {}", row.line));
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let Some(value) = row.value else {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(row.expression)
                                .monospace()
                                .size(theme::CODE_SIZE)
                                .color(theme::TEXT_MUTED),
                        )
                        .truncate(),
                    )
                    .on_hover_text("Derived from other parameters; edit those instead.");
                    return;
                };

                let drafting = self
                    .draft
                    .as_ref()
                    .is_some_and(|(name, _)| name == row.name);
                let mut shown = match &self.draft {
                    Some((_, draft)) if drafting => *draft,
                    _ => value,
                };
                let response = ui.add_enabled(
                    enabled,
                    egui::DragValue::new(&mut shown)
                        .speed(drag_speed(value))
                        .max_decimals(4),
                );
                if response.changed() {
                    self.draft = Some((row.name.to_string(), shown));
                }
                // A drag that has come to rest, or a typed value the field
                // has lost: one edit, one design step — not one per pixel.
                if drafting && (response.drag_stopped() || response.lost_focus()) {
                    let (_, draft) = self.draft.take().unwrap_or((String::new(), value));
                    if draft != value {
                        committed = Some(draft);
                    }
                }
            });
        });
        committed
    }
}

/// Width kept for the value field at the right of each row, so a long
/// name is cut short rather than pushing the value out of the panel.
const VALUE_WIDTH: f32 = 80.0;

/// A drag step proportional to the value, so a 200 mm plate and a 0.4 mm
/// clearance are both draggable.
fn drag_speed(value: f64) -> f64 {
    let magnitude = value.abs();
    if magnitude >= 100.0 {
        0.5
    } else if magnitude >= 1.0 {
        0.1
    } else {
        0.01
    }
}

#[cfg(test)]
mod tests {
    use super::{ParameterRow, ParametersPanel, ParametersView, drag_speed};

    /// Lay the panel out in a column of the given width and return the
    /// width the rows actually claimed.
    fn claimed_width(column_width: f32, rows: &[ParameterRow<'_>]) -> f32 {
        let context = egui::Context::default();
        crate::theme::apply(&context);
        let mut panel = ParametersPanel::default();
        let mut claimed = 0.0;
        // Two passes: the first frame lays fonts out; the second is stable.
        for _ in 0..2 {
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(column_width, 400.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ctx, |ui| {
                            panel.show(
                                ui,
                                ParametersView {
                                    parameters: rows,
                                    has_script: true,
                                    controls_enabled: true,
                                },
                            );
                            claimed = ui.min_rect().width();
                        });
                },
            );
        }
        claimed
    }

    /// A row that would be wider than its column is cut to fit: the panel
    /// beside the viewport keeps its width and the viewport starts at its
    /// edge, with no void between them.
    #[test]
    fn long_rows_stay_within_the_column() {
        let rows = [
            ParameterRow {
                name: "printer_build_volume_usable_height_after_brim_and_purge_tower",
                value: Some(250.0),
                expression: "250.0",
                line: 3,
            },
            ParameterRow {
                name: "clip_height",
                value: None,
                expression: "printer_build_volume_z - 2 * flange_thickness - mating_flange_thickness",
                line: 4,
            },
        ];
        let column_width = 240.0;
        let claimed = claimed_width(column_width, &rows);
        assert!(
            claimed <= column_width,
            "rows claimed {claimed} of a {column_width} column"
        );
    }

    #[test]
    fn the_drag_step_follows_the_size_of_the_value() {
        assert!(drag_speed(0.4) < drag_speed(20.0));
        assert!(drag_speed(20.0) < drag_speed(200.0));
        assert_eq!(drag_speed(-250.0), drag_speed(250.0));
    }
}
