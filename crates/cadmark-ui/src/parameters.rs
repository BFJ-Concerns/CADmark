// Parameters panel — the script's named numbers, editable in place.
//
// Adjusting a dimension is the one change the user makes without asking
// the AI (C16), so the panel sits beside the viewport rather than behind
// a mode, as one tab of the left panel: a row per module-level numeric
// name, a drag-or-type field for the ones bound to a literal, and the
// expression itself for the ones derived from other parameters.
// Committing a value is what the application turns into a rewritten
// script, a rebuild and a design step.
//
// Each row also carries a lock. A locked parameter is a hard constraint
// the user has fixed — a fit, a mounting position, an overall size — which
// the AI may change only with the user's explicit permission; the lock is
// a `# locked` comment on the parameter's line in the script, so it shows
// in the code panel and survives every rewrite. The user's own edits are
// never blocked by it: the lock binds the AI, not the person who set it.

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
    /// Whether the script marks this parameter `# locked`.
    pub locked: bool,
    /// The reason the marker gives, when it gives one.
    pub lock_reason: Option<&'a str>,
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
    /// Mark this parameter locked, or unmark it, in the script.
    SetLocked {
        name: String,
        locked: bool,
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
                    match self.show_row(ui, row, view.controls_enabled) {
                        RowEdit::None => {}
                        RowEdit::Value(value) => {
                            action = ParametersAction::Commit {
                                name: row.name.to_string(),
                                value,
                            };
                        }
                        RowEdit::Locked(locked) => {
                            action = ParametersAction::SetLocked {
                                name: row.name.to_string(),
                                locked,
                            };
                        }
                    }
                }
            });
        action
    }

    /// One row, and what the user did to it.
    fn show_row(&mut self, ui: &mut egui::Ui, row: &ParameterRow<'_>, enabled: bool) -> RowEdit {
        let mut edit = RowEdit::None;
        ui.horizontal(|ui| {
            // The row never grows past its column: a name that does not
            // fit is cut short (egui shows the whole of a truncated label
            // on hover), and the value keeps a fixed share at the right.
            let name_width = (ui.available_width() - VALUE_WIDTH - LOCK_WIDTH).max(0.0);
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
                // The lock sits at the row's right edge, past the value.
                let (glyph, colour, hint) = if row.locked {
                    (
                        LOCKED_GLYPH,
                        theme::TEXT,
                        match row.lock_reason {
                            Some(reason) => format!(
                                "Locked: {reason}. The AI may not change this without your \
                                 say-so; click to unlock."
                            ),
                            None => "Locked: the AI may not change this without your say-so; \
                                     click to unlock."
                                .to_string(),
                        },
                    )
                } else {
                    (
                        UNLOCKED_GLYPH,
                        theme::TEXT_MUTED,
                        "Click to lock: the AI will keep this value unless you allow a change."
                            .to_string(),
                    )
                };
                let lock = ui.add_enabled(
                    enabled,
                    egui::Button::new(
                        egui::RichText::new(glyph)
                            .size(theme::SMALL_SIZE)
                            .color(colour),
                    )
                    .frame(false)
                    .min_size(egui::vec2(LOCK_WIDTH, 0.0)),
                );
                if lock.on_hover_text(hint).clicked() {
                    edit = RowEdit::Locked(!row.locked);
                }

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
                        edit = RowEdit::Value(draft);
                    }
                }
            });
        });
        edit
    }
}

/// What one row's frame produced.
enum RowEdit {
    None,
    /// The user finished editing the value.
    Value(f64),
    /// The user clicked the lock; the new state.
    Locked(bool),
}

/// Width kept for the value field at the right of each row, so a long
/// name is cut short rather than pushing the value out of the panel.
const VALUE_WIDTH: f32 = 80.0;

/// Width of the lock toggle at the row's edge.
const LOCK_WIDTH: f32 = 18.0;

/// A closed padlock, from egui's bundled emoji font.
const LOCKED_GLYPH: &str = "\u{1F512}";
/// An open padlock.
const UNLOCKED_GLYPH: &str = "\u{1F513}";

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
                locked: true,
                lock_reason: Some("the printer's height"),
            },
            ParameterRow {
                name: "clip_height",
                value: None,
                expression: "printer_build_volume_z - 2 * flange_thickness - mating_flange_thickness",
                line: 4,
                locked: false,
                lock_reason: None,
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
