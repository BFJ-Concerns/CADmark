// View gizmo — the axis triad in the viewport corner. It shows which way
// the model's X, Y and Z run on screen, a click on an axis cap turns the
// camera to look along that axis — the +Y cap gives the front view, looking
// along +Y from the model's front (a second click flips to the opposite
// side) — and a drag on the gizmo orbits the view.

use crate::theme;

/// What the user did to the gizmo this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GizmoAction {
    None,
    /// Turn the camera to look at the model from this world direction.
    LookFrom([f32; 3]),
    /// Orbit the camera by this screen-space delta.
    Orbit(egui::Vec2),
}

/// Diameter of the gizmo's interactive disc, in points.
const SIZE: f32 = 96.0;
/// Distance from the viewport's corner.
const MARGIN: f32 = 12.0;
/// Radius of a positive axis cap; negative caps are slightly smaller.
const CAP_RADIUS: f32 = 9.0;

/// Colour of each world axis: the X red, Y green, Z blue that every CAD
/// package shares.
const AXIS_COLOURS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(232, 88, 92),
    egui::Color32::from_rgb(120, 196, 88),
    egui::Color32::from_rgb(78, 140, 245),
];
const AXIS_LABELS: [&str; 3] = ["X", "Y", "Z"];

/// One end of an axis, positioned on screen.
struct Cap {
    axis: usize,
    positive: bool,
    /// Offset from the gizmo centre in points, screen-down y.
    offset: egui::Vec2,
    /// Towards the viewer, -1..1, for draw order and shading.
    depth: f32,
}

impl Cap {
    fn direction(&self) -> [f32; 3] {
        let mut direction = [0.0; 3];
        direction[self.axis] = if self.positive { 1.0 } else { -1.0 };
        direction
    }

    /// The world direction a click on this cap makes the camera look
    /// along: the cap's own direction, or, when the view already looks
    /// along it, the opposite one, so a second click sees the other side.
    fn look_along(&self) -> [f32; 3] {
        let direction = self.direction();
        if self.depth < -0.999 {
            direction.map(|component| -component)
        } else {
            direction
        }
    }

    /// Where the camera stands to look along this cap's axis: the end
    /// opposite the direction of view.
    fn look_from_target(&self) -> [f32; 3] {
        self.look_along().map(|component| -component)
    }

    fn radius(&self) -> f32 {
        let scale = 0.85 + 0.15 * self.depth;
        if self.positive {
            CAP_RADIUS * scale
        } else {
            CAP_RADIUS * 0.8 * scale
        }
    }
}

/// Lay the six axis caps out around the centre, furthest from the viewer
/// first so drawing them in order stacks them correctly.
fn layout_caps(axes: [[f32; 3]; 3], reach: f32) -> Vec<Cap> {
    let mut caps: Vec<Cap> = (0..3)
        .flat_map(|axis| {
            let screen = axes[axis];
            [true, false].map(move |positive| {
                let sign = if positive { 1.0 } else { -1.0 };
                Cap {
                    axis,
                    positive,
                    offset: egui::vec2(screen[0], -screen[1]) * reach * sign,
                    depth: screen[2] * sign,
                }
            })
        })
        .collect();
    caps.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    caps
}

/// Draw the gizmo in the top-right corner of `viewport` and report any
/// interaction. `axes` are the screen-space images of world X, Y and Z:
/// x across the screen, y up it, z towards the viewer, each unit length
/// (the first three rows of the view matrix's columns).
pub fn show(ui: &mut egui::Ui, viewport: egui::Rect, axes: [[f32; 3]; 3]) -> GizmoAction {
    let origin = egui::pos2(viewport.right() - MARGIN - SIZE, viewport.top() + MARGIN);
    let mut action = GizmoAction::None;

    egui::Area::new(egui::Id::new("view_gizmo"))
        .order(egui::Order::Middle)
        .fixed_pos(origin)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(SIZE, SIZE), egui::Sense::click_and_drag());
            let centre = rect.center();
            let reach = SIZE * 0.5 - CAP_RADIUS - 2.0;
            let caps = layout_caps(axes, reach);

            let hovered_cap = response.hover_pos().and_then(|pos| {
                caps.iter()
                    .rev()
                    .find(|cap| (centre + cap.offset).distance(pos) <= cap.radius() + 2.0)
                    .map(|cap| (cap.axis, cap.positive))
            });

            if response.dragged_by(egui::PointerButton::Primary) {
                action = GizmoAction::Orbit(response.drag_delta());
            } else if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
                && let Some(cap) = caps
                    .iter()
                    .rev()
                    .find(|cap| (centre + cap.offset).distance(pos) <= cap.radius() + 2.0)
            {
                action = GizmoAction::LookFrom(cap.look_from_target());
            }

            let painter = ui.painter();
            if response.hovered() || response.dragged() {
                painter.circle_filled(centre, SIZE * 0.5, theme::TEXT_STRONG.gamma_multiply(0.08));
            }

            // Positive axis lines run under the caps that end them.
            for cap in caps.iter().filter(|cap| cap.positive) {
                painter.line_segment(
                    [centre, centre + cap.offset],
                    egui::Stroke::new(2.0_f32, shade(AXIS_COLOURS[cap.axis], cap.depth)),
                );
            }

            for cap in &caps {
                let colour = shade(AXIS_COLOURS[cap.axis], cap.depth);
                let position = centre + cap.offset;
                let radius = cap.radius();
                let is_hovered = hovered_cap == Some((cap.axis, cap.positive));
                let fill = if is_hovered {
                    colour.gamma_multiply(1.25)
                } else {
                    colour
                };
                if cap.positive {
                    painter.circle_filled(position, radius, fill);
                    painter.text(
                        position,
                        egui::Align2::CENTER_CENTER,
                        AXIS_LABELS[cap.axis],
                        egui::FontId::proportional(11.0),
                        theme::SUNKEN,
                    );
                } else {
                    painter.circle(
                        position,
                        radius,
                        theme::VIEWPORT.gamma_multiply(0.9),
                        egui::Stroke::new(1.5_f32, fill),
                    );
                    if is_hovered {
                        painter.text(
                            position,
                            egui::Align2::CENTER_CENTER,
                            format!("-{}", AXIS_LABELS[cap.axis]),
                            egui::FontId::proportional(9.0),
                            fill,
                        );
                    }
                }
            }

            if let Some((axis, positive)) = hovered_cap {
                let cap = caps
                    .iter()
                    .find(|cap| cap.axis == axis && cap.positive == positive)
                    .expect("hovered cap is one of the six laid out");
                let along = cap.look_along();
                let sign = if along[axis] > 0.0 { "+" } else { "-" };
                response
                    .clone()
                    .on_hover_text(format!("Look along {sign}{}", AXIS_LABELS[axis]));
            } else if response.hovered() {
                response.clone().on_hover_text("Drag to orbit");
            }
        });

    action
}

/// Dim an axis colour as its cap turns away from the viewer.
fn shade(colour: egui::Color32, depth: f32) -> egui::Color32 {
    colour.gamma_multiply(0.7 + 0.3 * ((depth + 1.0) * 0.5))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_are_ordered_back_to_front() {
        // Z straight at the viewer: +Z is nearest, -Z furthest.
        let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let caps = layout_caps(axes, 30.0);
        assert!(!caps.first().unwrap().positive);
        assert_eq!(caps.first().unwrap().axis, 2);
        assert!(caps.last().unwrap().positive);
        assert_eq!(caps.last().unwrap().axis, 2);
    }

    #[test]
    fn clicking_a_cap_looks_along_its_axis() {
        // Looking down -Z: +Y points up the screen, +X right.
        let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let caps = layout_caps(axes, 30.0);
        let plus_y = caps
            .iter()
            .find(|cap| cap.axis == 1 && cap.positive)
            .unwrap();
        // The front view: camera at -Y looking along +Y.
        assert_eq!(plus_y.look_along(), [0.0, 1.0, 0.0]);
        assert_eq!(plus_y.look_from_target(), [0.0, -1.0, 0.0]);
        let minus_x = caps
            .iter()
            .find(|cap| cap.axis == 0 && !cap.positive)
            .unwrap();
        assert_eq!(minus_x.look_from_target(), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn clicking_the_cap_already_looked_along_flips_to_the_far_side() {
        // Looking down -Z, so the view already looks along -Z: -Z points
        // away from the viewer and +Z towards them.
        let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let caps = layout_caps(axes, 30.0);
        let minus_z = caps
            .iter()
            .find(|cap| cap.axis == 2 && !cap.positive)
            .unwrap();
        assert_eq!(minus_z.look_along(), [0.0, 0.0, 1.0]);
        assert_eq!(minus_z.look_from_target(), [0.0, 0.0, -1.0]);
        // The facing cap is not yet looked along, so it turns the view
        // right round to look along it.
        let plus_z = caps
            .iter()
            .find(|cap| cap.axis == 2 && cap.positive)
            .unwrap();
        assert_eq!(plus_z.look_from_target(), [0.0, 0.0, -1.0]);
    }

    #[test]
    fn caps_sit_on_screen_axes_with_y_flipped() {
        let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let caps = layout_caps(axes, 30.0);
        let plus_x = caps
            .iter()
            .find(|cap| cap.axis == 0 && cap.positive)
            .unwrap();
        assert_eq!(plus_x.offset, egui::vec2(30.0, 0.0));
        // Screen y runs down, so world up is a negative offset.
        let plus_y = caps
            .iter()
            .find(|cap| cap.axis == 1 && cap.positive)
            .unwrap();
        assert_eq!(plus_y.offset, egui::vec2(0.0, -30.0));
        assert_eq!(plus_y.direction(), [0.0, 1.0, 0.0]);
        let minus_y = caps
            .iter()
            .find(|cap| cap.axis == 1 && !cap.positive)
            .unwrap();
        assert_eq!(minus_y.direction(), [0.0, -1.0, 0.0]);
    }
}
