// View cube — the navigation control in the viewport corner. A cube
// labelled with the six standard views turns with the model, so it always
// shows which way the model faces. Its edges and corners are chamfered
// into flat facets, and every facet is a click target: a face looks
// square at that side, an edge facet looks from the 45° between two
// sides, and a corner facet looks from the three-way diagonal. Dragging
// the cube orbits, the curved arrows beside it roll the view a quarter
// turn, and the row beneath fits the model, switches projection, and
// shows or hides the axis triad.

use std::sync::LazyLock;

use crate::theme;

/// What the user did to the view cube this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewCubeAction {
    None,
    /// Turn the camera to look at the model from this world direction.
    LookFrom([f32; 3]),
    /// Orbit the camera by this screen-space delta.
    Orbit(egui::Vec2),
    /// Turn the view a quarter turn about the line of sight.
    Roll {
        clockwise: bool,
    },
    /// Frame the whole model.
    Fit,
    /// Switch between perspective and orthographic projection.
    ToggleProjection,
    /// Show or hide the axis triad.
    ToggleAxes,
}

/// What the cube's controls reflect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewCubeState {
    pub orthographic: bool,
    pub axes_shown: bool,
}

/// Side of the cube canvas, in points. The cube's silhouette reaches at
/// most `HALF_SIZE * sqrt(3)` from the centre, which leaves the corners
/// free for the roll arrows.
const CANVAS: f32 = 136.0;
/// Half the cube's edge, in points.
const HALF_SIZE: f32 = 30.0;
/// Half-width of a flat face as a fraction of `HALF_SIZE`; the rest of
/// each edge is the chamfer.
const FACE_EXTENT: f32 = 0.64;
/// Distance from the viewport's corner.
const MARGIN: f32 = 12.0;
/// Radius of a roll arrow's arc, and how far its centre sits in from the
/// canvas corner.
const ARROW_RADIUS: f32 = 9.0;
const ARROW_INSET: f32 = 15.0;
/// How near the pointer must be to an arrow's centre to press it.
const ARROW_HIT_RADIUS: f32 = 14.0;
/// Length of an axis-triad arm as a multiple of the cube's edge.
const TRIAD_REACH: f32 = 1.6;
/// Fraction of a flat face's width its label spans.
const LABEL_SPAN: f32 = 0.86;

/// Colour of each world axis: the X red, Y green, Z blue that every CAD
/// package shares.
const AXIS_COLOURS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(232, 88, 92),
    egui::Color32::from_rgb(120, 196, 88),
    egui::Color32::from_rgb(78, 140, 245),
];
const AXIS_LABELS: [&str; 3] = ["X", "Y", "Z"];

/// The cube's facets are drawn in this grey, shaded by how squarely each
/// one faces the viewer.
const FACET_COLOUR: egui::Color32 = egui::Color32::from_rgb(178, 182, 190);

/// One labelled side of the cube in world space.
struct Face {
    /// Outward normal; also the direction the face looks from.
    normal: [f32; 3],
    /// Runs rightward across the face when it is viewed square with
    /// `up` upward, so `(across, up, normal)` is right-handed.
    across: [f32; 3],
    /// The way the face's label reads upward.
    up: [f32; 3],
    label: &'static str,
}

const FACES: [Face; 6] = [
    Face {
        normal: [0.0, -1.0, 0.0],
        across: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        label: "Front",
    },
    Face {
        normal: [0.0, 1.0, 0.0],
        across: [-1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        label: "Back",
    },
    Face {
        normal: [-1.0, 0.0, 0.0],
        across: [0.0, -1.0, 0.0],
        up: [0.0, 0.0, 1.0],
        label: "Left",
    },
    Face {
        normal: [1.0, 0.0, 0.0],
        across: [0.0, 1.0, 0.0],
        up: [0.0, 0.0, 1.0],
        label: "Right",
    },
    Face {
        normal: [0.0, 0.0, 1.0],
        across: [1.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        label: "Top",
    },
    Face {
        normal: [0.0, 0.0, -1.0],
        across: [-1.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        label: "Bottom",
    },
];

/// One flat facet of the chamfered cube: a face, an edge chamfer, or a
/// corner chamfer. Its outward normal is the direction a click looks
/// from.
struct Facet {
    direction: [f32; 3],
    /// World-space outline, in the cube's unit frame.
    vertices: Vec<[f32; 3]>,
    /// The labelled side this facet is, when it is one.
    face: Option<&'static Face>,
}

/// The twenty-six facets: six square faces of half-width `FACE_EXTENT`,
/// twelve rectangles chamfering the edges between them, and eight
/// triangles chamfering the corners.
static FACETS: LazyLock<Vec<Facet>> = LazyLock::new(|| {
    let k = FACE_EXTENT;
    let mut facets = Vec::with_capacity(26);

    for face in &FACES {
        let corner = |a: f32, u: f32| point(face.normal, 1.0, face.across, a * k, face.up, u * k);
        facets.push(Facet {
            direction: face.normal,
            vertices: vec![
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            ],
            face: Some(face),
        });
    }

    for (index, a) in FACES.iter().enumerate() {
        for b in &FACES[index + 1..] {
            if dot(a.normal, b.normal).abs() > 1e-6 {
                continue;
            }
            let along = cross(a.normal, b.normal);
            facets.push(Facet {
                direction: normalize(add(a.normal, b.normal)),
                vertices: vec![
                    point(a.normal, 1.0, b.normal, k, along, -k),
                    point(a.normal, 1.0, b.normal, k, along, k),
                    point(b.normal, 1.0, a.normal, k, along, k),
                    point(b.normal, 1.0, a.normal, k, along, -k),
                ],
                face: None,
            });
        }
    }

    for signs in [
        [1.0, 1.0, 1.0],
        [1.0, 1.0, -1.0],
        [1.0, -1.0, 1.0],
        [1.0, -1.0, -1.0],
        [-1.0, 1.0, 1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [-1.0, -1.0, -1.0],
    ] {
        let [x, y, z] = signs;
        facets.push(Facet {
            direction: normalize(signs),
            vertices: vec![[x, y * k, z * k], [x * k, y, z * k], [x * k, y * k, z]],
            face: None,
        });
    }

    facets
});

/// `a * s + b * t + c * u`, a point built from three axis directions.
fn point(a: [f32; 3], s: f32, b: [f32; 3], t: f32, c: [f32; 3], u: f32) -> [f32; 3] {
    std::array::from_fn(|axis| a[axis] * s + b[axis] * t + c[axis] * u)
}

/// The name of the view a direction looks from: the faces it leans
/// towards, front or back first, then top or bottom, then left or right.
fn view_name(direction: [f32; 3]) -> String {
    let names = [
        (1, "front", "back"),
        (2, "bottom", "top"),
        (0, "left", "right"),
    ];
    let mut parts: Vec<&str> = Vec::new();
    for (axis, negative, positive) in names {
        if direction[axis] < -0.1 {
            parts.push(negative);
        } else if direction[axis] > 0.1 {
            parts.push(positive);
        }
    }
    let mut name = parts.join("-");
    if let Some(first) = name.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    name
}

/// The cube as it appears on screen for one orientation of the view.
struct CubeOnScreen {
    /// Screen images of world X, Y and Z: x across the screen, y up it,
    /// z towards the viewer, each unit length.
    axes: [[f32; 3]; 3],
}

impl CubeOnScreen {
    /// Project a world vector: screen x, screen y (up), and depth towards
    /// the viewer.
    fn project(&self, world: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|component| {
            (0..3)
                .map(|axis| world[axis] * self.axes[axis][component])
                .sum()
        })
    }

    /// Screen offset from the cube's centre of a world point, in points
    /// with screen-down y.
    fn offset(&self, world: [f32; 3]) -> egui::Vec2 {
        let [x, y, _] = self.project(world);
        egui::vec2(x, -y) * HALF_SIZE
    }

    /// How squarely a facet meets the viewer: 1 face on, 0 edge on,
    /// negative when it faces away.
    fn facing(&self, facet: &Facet) -> f32 {
        self.project(facet.direction)[2]
    }

    /// The facets the viewer can see, least square-on first, so drawing
    /// them in order leaves the most prominent facet's outline on top.
    /// The cube is convex, so the visible facets never overlap on screen.
    fn visible_facets(&self) -> Vec<&'static Facet> {
        let mut facets: Vec<&Facet> = FACETS
            .iter()
            .filter(|facet| self.facing(facet) > 1e-4)
            .collect();
        facets.sort_by(|a, b| self.facing(a).total_cmp(&self.facing(b)));
        facets
    }

    /// A facet's outline on screen, as offsets from the cube's centre.
    fn outline(&self, facet: &Facet) -> Vec<egui::Vec2> {
        facet
            .vertices
            .iter()
            .map(|vertex| self.offset(*vertex))
            .collect()
    }

    /// The facet under a pointer offset from the cube's centre, in
    /// points with screen-down y.
    fn hit(&self, offset: egui::Vec2) -> Option<&'static Facet> {
        self.visible_facets()
            .into_iter()
            .find(|facet| inside_convex(&self.outline(facet), offset))
    }

    /// Where a point on a face's flat square lands on screen, as an
    /// offset from the cube's centre: `(across, up)` in -1..1 across
    /// the square.
    fn on_face(&self, face: &Face, across: f32, up: f32) -> egui::Vec2 {
        self.offset(point(
            face.normal,
            1.0,
            face.across,
            across * FACE_EXTENT,
            face.up,
            up * FACE_EXTENT,
        ))
    }
}

/// Whether a point lies inside a convex polygon, whichever way round
/// its vertices run.
fn inside_convex(polygon: &[egui::Vec2], point: egui::Vec2) -> bool {
    let mut sign = 0.0_f32;
    for (index, start) in polygon.iter().enumerate() {
        let end = polygon[(index + 1) % polygon.len()];
        let edge = end - *start;
        let towards = point - *start;
        let side = edge.x * towards.y - edge.y * towards.x;
        if side.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = side.signum();
        } else if side.signum() != sign {
            return false;
        }
    }
    sign != 0.0
}

/// Draw the view cube in the top-right corner of `viewport` and report
/// any interaction. `axes` are the screen-space images of world X, Y and
/// Z: x across the screen, y up it, z towards the viewer, each unit
/// length (the first three rows of the view matrix's columns).
pub fn show(
    ui: &mut egui::Ui,
    viewport: egui::Rect,
    axes: [[f32; 3]; 3],
    state: ViewCubeState,
) -> ViewCubeAction {
    let origin = egui::pos2(viewport.right() - MARGIN - CANVAS, viewport.top() + MARGIN);
    let mut action = ViewCubeAction::None;

    egui::Area::new(egui::Id::new("view_cube"))
        .order(egui::Order::Middle)
        .fixed_pos(origin)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
            ui.vertical(|ui| {
                action = show_cube(ui, axes, state.axes_shown);
                let row = egui::Layout::left_to_right(egui::Align::Center)
                    .with_main_align(egui::Align::Center);
                ui.allocate_ui_with_layout(egui::vec2(CANVAS, 22.0), row, |ui| {
                    if ui
                        .add(egui::Button::new("\u{22A1} Fit").small())
                        .on_hover_text("Frame the whole model\nF")
                        .clicked()
                    {
                        action = ViewCubeAction::Fit;
                    }
                    let (projection, other) = if state.orthographic {
                        ("Ortho", "perspective")
                    } else {
                        ("Persp", "orthographic")
                    };
                    if ui
                        .add(egui::Button::new(projection).small())
                        .on_hover_text(format!("Switch to {other}\nP"))
                        .clicked()
                    {
                        action = ViewCubeAction::ToggleProjection;
                    }
                    if ui
                        .add(egui::Button::new("XYZ").small().selected(state.axes_shown))
                        .on_hover_text(if state.axes_shown {
                            "Hide the axes"
                        } else {
                            "Show which way X, Y and Z run"
                        })
                        .clicked()
                    {
                        action = ViewCubeAction::ToggleAxes;
                    }
                });
            });
        });

    action
}

/// The cube canvas: the cube itself, the roll arrows, and the triad.
fn show_cube(ui: &mut egui::Ui, axes: [[f32; 3]; 3], axes_shown: bool) -> ViewCubeAction {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(CANVAS, CANVAS), egui::Sense::click_and_drag());
    let centre = rect.center();
    let cube = CubeOnScreen { axes };
    let mut action = ViewCubeAction::None;

    let arrows = [
        (
            rect.left_top() + egui::vec2(ARROW_INSET, ARROW_INSET),
            false,
        ),
        (
            rect.right_top() + egui::vec2(-ARROW_INSET, ARROW_INSET),
            true,
        ),
    ];
    let arrow_at = |pos: egui::Pos2| {
        arrows
            .iter()
            .find(|(arrow_centre, _)| arrow_centre.distance(pos) <= ARROW_HIT_RADIUS)
            .map(|(_, clockwise)| *clockwise)
    };

    let hovered_arrow = response.hover_pos().and_then(arrow_at);
    let hovered_facet = response
        .hover_pos()
        .filter(|_| hovered_arrow.is_none())
        .and_then(|pos| cube.hit(pos - centre));

    if response.dragged_by(egui::PointerButton::Primary) {
        let delta = response.drag_delta();
        if delta != egui::Vec2::ZERO {
            action = ViewCubeAction::Orbit(delta);
        }
    } else if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        if let Some(clockwise) = arrow_at(pos) {
            action = ViewCubeAction::Roll { clockwise };
        } else if let Some(facet) = cube.hit(pos - centre) {
            action = ViewCubeAction::LookFrom(facet.direction);
        }
    }

    let painter = ui.painter();
    let active = response.hovered() || response.dragged();

    if axes_shown {
        paint_triad(painter, centre, &cube);
    }

    for facet in cube.visible_facets() {
        let facing = cube.facing(facet);
        let is_hovered = hovered_facet.is_some_and(|hovered| std::ptr::eq(hovered, facet));
        let fill = if is_hovered {
            theme::ACCENT.gamma_multiply(0.85)
        } else {
            FACET_COLOUR.gamma_multiply(0.5 + 0.5 * facing)
        };
        let outline: Vec<egui::Pos2> = cube
            .outline(facet)
            .into_iter()
            .map(|offset| centre + offset)
            .collect();
        painter.add(egui::Shape::convex_polygon(
            outline.clone(),
            fill,
            egui::Stroke::NONE,
        ));
        if let Some(face) = facet.face {
            paint_face_label(ui, centre, &cube, face);
        }
        painter.add(egui::Shape::closed_line(
            outline,
            egui::Stroke::new(1.0_f32, theme::SUNKEN.gamma_multiply(0.8)),
        ));
    }

    if active {
        for (arrow_centre, clockwise) in arrows {
            let colour = if hovered_arrow == Some(clockwise) {
                theme::ACCENT
            } else {
                theme::TEXT
            };
            paint_roll_arrow(painter, arrow_centre, clockwise, colour);
        }
    }

    if let Some(facet) = hovered_facet {
        response.clone().on_hover_text(format!(
            "Look from the {}",
            view_name(facet.direction).to_lowercase()
        ));
    } else if let Some(clockwise) = hovered_arrow {
        let way = if clockwise {
            "clockwise"
        } else {
            "anticlockwise"
        };
        response
            .clone()
            .on_hover_text(format!("Turn the view {way}"));
    } else if response.hovered() {
        response.clone().on_hover_text("Drag to orbit");
    }

    action
}

/// Paint a face's label lying on the face: the text is laid out flat,
/// then each vertex is carried onto the face's screen image, so the
/// label foreshortens and turns with the face like a decal.
fn paint_face_label(ui: &egui::Ui, centre: egui::Pos2, cube: &CubeOnScreen, face: &Face) {
    let ctx = ui.ctx();
    let galley = ctx.fonts(|fonts| {
        fonts.layout_no_wrap(
            face.label.to_string(),
            egui::FontId::proportional(13.0),
            theme::SUNKEN,
        )
    });
    let text_size = galley.size();
    if text_size.x <= 0.0 {
        return;
    }
    let shape = egui::Shape::galley(egui::Pos2::ZERO, galley, theme::SUNKEN);
    let clip_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, text_size);
    let primitives = ctx.tessellate(
        vec![egui::epaint::ClippedShape { clip_rect, shape }],
        ctx.pixels_per_point(),
    );

    // Text points map to face coordinates: the label's width spans
    // `LABEL_SPAN` of the flat face, and its height keeps the glyphs'
    // aspect.
    let scale = LABEL_SPAN * 2.0 / text_size.x;
    for primitive in primitives {
        let egui::epaint::Primitive::Mesh(mut mesh) = primitive.primitive else {
            continue;
        };
        for vertex in &mut mesh.vertices {
            let across = (vertex.pos.x - text_size.x * 0.5) * scale;
            let up = -(vertex.pos.y - text_size.y * 0.5) * scale;
            vertex.pos = centre + cube.on_face(face, across, up);
        }
        ui.painter().add(egui::Shape::mesh(mesh));
    }
}

/// A curved arrow around `centre` showing a quarter turn the given way.
fn paint_roll_arrow(
    painter: &egui::Painter,
    centre: egui::Pos2,
    clockwise: bool,
    colour: egui::Color32,
) {
    // The arc runs over the top of its centre; the head points down the
    // side it turns towards.
    let sweep = std::f32::consts::PI * 1.1;
    let start = -std::f32::consts::FRAC_PI_2 - sweep * 0.5;
    let steps = 14;
    let points: Vec<egui::Pos2> = (0..=steps)
        .map(|step| {
            let fraction = step as f32 / steps as f32;
            let angle = start + sweep * if clockwise { fraction } else { 1.0 - fraction };
            centre + egui::vec2(angle.cos(), angle.sin()) * ARROW_RADIUS
        })
        .collect();
    let stroke = egui::Stroke::new(1.8_f32, colour);
    let tip = *points.last().expect("arc has points");
    let before_tip = points[points.len() - 2];
    painter.add(egui::Shape::line(points, stroke));

    let tangent = (tip - before_tip).normalized();
    let normal = egui::vec2(-tangent.y, tangent.x);
    let head = 5.0;
    painter.add(egui::Shape::convex_polygon(
        vec![
            tip + tangent * head * 0.6,
            tip - tangent * head * 0.5 + normal * head * 0.55,
            tip - tangent * head * 0.5 - normal * head * 0.55,
        ],
        colour,
        egui::Stroke::NONE,
    ));
}

/// The axis triad: three labelled arms from the cube's origin corner
/// along the world axes as they lie on screen, the way the model's own
/// axes would leave its corner. Arms pointing away from the viewer are
/// drawn first and dimmer.
fn paint_triad(painter: &egui::Painter, centre: egui::Pos2, cube: &CubeOnScreen) {
    let origin = centre + cube.offset([-1.0, -1.0, -1.0]);
    let mut arms: Vec<(usize, [f32; 3])> = (0..3)
        .map(|axis| {
            let mut direction = [0.0; 3];
            direction[axis] = 1.0;
            (axis, cube.project(direction))
        })
        .collect();
    arms.sort_by(|a, b| a.1[2].total_cmp(&b.1[2]));
    let reach = HALF_SIZE * 2.0 * TRIAD_REACH;
    for (axis, [x, y, depth]) in arms {
        let colour = AXIS_COLOURS[axis].gamma_multiply(0.65 + 0.35 * (depth + 1.0) * 0.5);
        let along = egui::vec2(x, -y);
        let tip = origin + along * reach;
        painter.line_segment([origin, tip], egui::Stroke::new(1.5_f32, colour));
        painter.text(
            origin + along * (reach + 8.0),
            egui::Align2::CENTER_CENTER,
            AXIS_LABELS[axis],
            egui::FontId::proportional(11.0),
            colour,
        );
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|axis| a[axis] + b[axis])
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length < 1e-10 {
        return [0.0; 3];
    }
    v.map(|component| component / length)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The front view: X right, Z up, Y away from the viewer.
    fn front_view() -> CubeOnScreen {
        CubeOnScreen {
            axes: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]],
        }
    }

    fn face(label: &str) -> &'static Face {
        FACES.iter().find(|face| face.label == label).unwrap()
    }

    fn approx(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|axis| (a[axis] - b[axis]).abs() < 1e-5)
    }

    #[test]
    fn every_face_is_right_handed_with_its_label_upright() {
        for face in &FACES {
            assert!(
                approx(cross(face.across, face.up), face.normal),
                "{}",
                face.label
            );
        }
    }

    #[test]
    fn the_cube_has_six_faces_twelve_edges_and_eight_corners() {
        assert_eq!(FACETS.len(), 26);
        assert_eq!(
            FACETS.iter().filter(|facet| facet.face.is_some()).count(),
            6
        );
        assert_eq!(
            FACETS
                .iter()
                .filter(|facet| facet.vertices.len() == 3)
                .count(),
            8
        );
        // Every facet's outline lies in the plane its direction faces.
        for facet in FACETS.iter() {
            let height = dot(facet.vertices[0], facet.direction);
            for vertex in &facet.vertices {
                assert!((dot(*vertex, facet.direction) - height).abs() < 1e-5);
            }
            assert!((dot(facet.direction, facet.direction) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn square_on_the_front_face_shows_with_its_chamfers_around_it() {
        let cube = front_view();
        let visible = cube.visible_facets();
        // The front face, its four edge chamfers and its four corners.
        assert_eq!(visible.len(), 9);
        let most_square = visible.last().unwrap();
        assert_eq!(most_square.face.map(|face| face.label), Some("Front"));
        assert!(
            visible
                .iter()
                .all(|facet| facet.face.map(|face| face.label) != Some("Back"))
        );
    }

    #[test]
    fn the_isometric_view_shows_three_faces() {
        // Looking from (1, -1, 1): right, front and top all show.
        let d = 1.0 / 3.0_f32.sqrt();
        let e = 1.0 / 2.0_f32.sqrt();
        let f = 1.0 / 6.0_f32.sqrt();
        let cube = CubeOnScreen {
            axes: [[e, -f, d], [e, f, -d], [0.0, 2.0 * f, d]],
        };
        let faces: Vec<&str> = cube
            .visible_facets()
            .iter()
            .filter_map(|facet| facet.face.map(|face| face.label))
            .collect();
        assert_eq!(faces.len(), 3);
        for label in ["Right", "Front", "Top"] {
            assert!(faces.contains(&label), "{faces:?}");
        }
    }

    #[test]
    fn hits_land_on_the_face_its_edge_chamfer_and_its_corner() {
        let cube = front_view();
        let hit = cube.hit(egui::vec2(0.0, 0.0)).unwrap();
        assert_eq!(hit.face.map(|face| face.label), Some("Front"));
        // Out past the flat face on the right: the chamfer towards the
        // right side, looking from front-right.
        let between = (FACE_EXTENT + 1.0) * 0.5 * HALF_SIZE;
        let hit = cube.hit(egui::vec2(between, 0.0)).unwrap();
        let e = 1.0 / 2.0_f32.sqrt();
        assert!(approx(hit.direction, [e, -e, 0.0]), "{:?}", hit.direction);
        // Diagonally out to the top-right: the corner chamfer. Screen up
        // is world +Z.
        let corner = (FACE_EXTENT + 0.08) * HALF_SIZE;
        let hit = cube.hit(egui::vec2(corner, -corner)).unwrap();
        let d = 1.0 / 3.0_f32.sqrt();
        assert!(approx(hit.direction, [d, -d, d]), "{:?}", hit.direction);
        // Beyond the chamfered silhouette, where a sharp corner would be.
        assert!(
            cube.hit(egui::vec2(HALF_SIZE * 0.95, -HALF_SIZE * 0.95))
                .is_none()
        );
        assert!(cube.hit(egui::vec2(HALF_SIZE * 1.2, 0.0)).is_none());
    }

    #[test]
    fn face_coordinates_land_on_the_flat_face_as_it_lies_on_screen() {
        let cube = front_view();
        let front = face("Front");
        let extent = HALF_SIZE * FACE_EXTENT;
        // Square on: across is screen right, up is screen up.
        assert_eq!(cube.on_face(front, 1.0, 0.0), egui::vec2(extent, 0.0));
        assert_eq!(cube.on_face(front, 0.0, 1.0), egui::vec2(0.0, -extent));
        // Rolled a quarter turn anticlockwise: world Z now runs to the
        // screen's left, so the face's up does too.
        let rolled = CubeOnScreen {
            axes: [[0.0, 1.0, 0.0], [0.0, 0.0, -1.0], [-1.0, 0.0, 0.0]],
        };
        assert_eq!(rolled.on_face(front, 0.0, 1.0), egui::vec2(-extent, 0.0));
    }

    #[test]
    fn convex_hit_testing_ignores_winding() {
        let square = [
            egui::vec2(-1.0, -1.0),
            egui::vec2(1.0, -1.0),
            egui::vec2(1.0, 1.0),
            egui::vec2(-1.0, 1.0),
        ];
        let reversed: Vec<egui::Vec2> = square.iter().rev().copied().collect();
        assert!(inside_convex(&square, egui::vec2(0.5, 0.5)));
        assert!(inside_convex(&reversed, egui::vec2(0.5, 0.5)));
        assert!(!inside_convex(&square, egui::vec2(1.5, 0.5)));
        assert!(!inside_convex(&reversed, egui::vec2(1.5, 0.5)));
    }

    #[test]
    fn view_names_list_the_faces_leaned_towards() {
        assert_eq!(view_name([0.0, -1.0, 0.0]), "Front");
        let e = 1.0 / 2.0_f32.sqrt();
        assert_eq!(view_name([e, -e, 0.0]), "Front-right");
        let d = 1.0 / 3.0_f32.sqrt();
        assert_eq!(view_name([-d, d, -d]), "Back-bottom-left");
    }
}
