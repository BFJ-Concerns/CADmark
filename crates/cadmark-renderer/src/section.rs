// The section plane: a half-space the viewport keeps and whose other side
// it throws away, so the user can look inside a part without exporting it.
//
// The plane is axis-aligned, sits at a position along that axis, and can be
// reversed so the other half is the one kept. Everything the shaders need
// travels as one `vec4` plane equation, which is also what makes the
// predicate testable without a GPU: `keeps` and `equation` are the same
// decision expressed twice, and a test holds them to each other.

use crate::camera::Bounds3;

/// The axis a section plane cuts along. The plane's normal lies on this
/// axis, so the plane itself is perpendicular to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    /// Index of this axis in a `[f32; 3]` position.
    pub fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Axis::X => "X",
            Axis::Y => "Y",
            Axis::Z => "Z",
        }
    }
}

/// A half-space clip. When `enabled`, geometry on the discarded side of the
/// plane is not drawn and not pickable; when not, nothing is clipped.
///
/// Unflipped, the kept side is the one with the *smaller* coordinate along
/// `axis`: a point is kept when `point[axis] <= offset`. Flipping negates
/// the whole plane equation, so the kept and discarded sides swap exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionPlane {
    pub enabled: bool,
    pub axis: Axis,
    /// Position of the plane along its axis, in model units.
    pub offset: f32,
    pub flipped: bool,
}

impl Default for SectionPlane {
    fn default() -> Self {
        Self {
            enabled: false,
            axis: Axis::X,
            offset: 0.0,
            flipped: false,
        }
    }
}

impl SectionPlane {
    /// Whether the section keeps `point`. A disabled plane keeps everything;
    /// a point exactly on the plane is kept whichever way it faces.
    pub fn keeps(&self, point: [f32; 3]) -> bool {
        let [nx, ny, nz, d] = self.equation();
        nx * point[0] + ny * point[1] + nz * point[2] + d >= 0.0
    }

    /// The plane as `[nx, ny, nz, d]`, where a point is kept when
    /// `dot(n, point) + d >= 0`. This is the form the shaders bind, and
    /// the disabled plane is all zeroes so their test keeps everything
    /// without needing a separate flag.
    pub fn equation(&self) -> [f32; 4] {
        if !self.enabled {
            return [0.0; 4];
        }

        // Unflipped: keep `point[axis] <= offset`, i.e. `-point[axis] + offset >= 0`.
        let sign = if self.flipped { 1.0 } else { -1.0 };
        let mut equation = [0.0f32; 4];
        equation[self.axis.index()] = sign;
        equation[3] = -sign * self.offset;
        equation
    }

    /// Reverse which half is kept.
    pub fn flip(&mut self) {
        self.flipped = !self.flipped;
    }

    /// Cut along `axis` from the middle of `bounds`. Re-centring on an axis
    /// change is what stops a plane carried over from another axis sitting
    /// clear of the model, where the user sees no cut and no reason for it.
    pub fn cut_along(&mut self, axis: Axis, bounds: Option<Bounds3>) {
        self.axis = axis;
        let (low, high) = travel_along(bounds, axis);
        self.offset = (low + high) * 0.5;
    }
}

/// How far the plane may travel along `axis`: the model's extent, widened a
/// little so the extremes clear the part entirely. Without bounds — no model
/// loaded — a symmetric fallback keeps the control usable rather than dead.
pub fn travel_along(bounds: Option<Bounds3>, axis: Axis) -> (f32, f32) {
    let Some(bounds) = bounds else {
        return (-10.0, 10.0);
    };
    let index = axis.index();
    let (low, high) = (bounds.min[index], bounds.max[index]);
    // A model flat on this axis would otherwise give a zero-width slider.
    let margin = ((high - low) * 0.02).max(1e-3);
    (low - margin, high + margin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(axis: Axis, offset: f32, flipped: bool) -> SectionPlane {
        SectionPlane {
            enabled: true,
            axis,
            offset,
            flipped,
        }
    }

    /// Evaluate a plane equation the way the shaders do, so a test failure
    /// here is a failure on the GPU too.
    fn shader_keeps(equation: [f32; 4], point: [f32; 3]) -> bool {
        equation[0] * point[0] + equation[1] * point[1] + equation[2] * point[2] + equation[3]
            >= 0.0
    }

    #[test]
    fn a_disabled_plane_keeps_every_point() {
        let plane = SectionPlane::default();
        assert!(!plane.enabled);
        for point in [[-1e6, 0.0, 0.0], [1e6, 0.0, 0.0], [0.0, 0.0, 0.0]] {
            assert!(plane.keeps(point), "disabled plane discarded {point:?}");
        }
        assert_eq!(plane.equation(), [0.0; 4]);
    }

    #[test]
    fn the_kept_side_is_the_low_side_until_the_plane_is_flipped() {
        let plane = section(Axis::X, 5.0, false);
        assert!(plane.keeps([4.0, 0.0, 0.0]));
        assert!(!plane.keeps([6.0, 0.0, 0.0]));

        let flipped = section(Axis::X, 5.0, true);
        assert!(!flipped.keeps([4.0, 0.0, 0.0]));
        assert!(flipped.keeps([6.0, 0.0, 0.0]));
    }

    #[test]
    fn flipping_swaps_every_point_off_the_plane() {
        // A flip that kept the same side would still render something
        // plausible, so assert the inversion pointwise on every axis.
        for axis in Axis::ALL {
            let plane = section(axis, 2.5, false);
            let mut flipped = plane;
            flipped.flip();
            assert!(flipped.flipped);

            for coordinate in [-10.0, -0.5, 0.0, 2.0, 2.499, 2.501, 3.0, 10.0f32] {
                let mut point = [0.0f32; 3];
                point[axis.index()] = coordinate;
                assert_ne!(
                    plane.keeps(point),
                    flipped.keeps(point),
                    "{axis:?} at {coordinate} was on the same side after a flip"
                );
            }
        }
    }

    #[test]
    fn moving_the_plane_moves_the_boundary_along_its_own_axis() {
        for axis in Axis::ALL {
            let mut point = [0.0f32; 3];
            point[axis.index()] = 3.0;

            assert!(
                section(axis, 4.0, false).keeps(point),
                "{axis:?} discarded a point below the plane"
            );
            assert!(
                !section(axis, 2.0, false).keeps(point),
                "{axis:?} kept a point the plane had moved past"
            );
        }
    }

    #[test]
    fn a_plane_on_one_axis_never_clips_along_another() {
        let plane = section(Axis::Z, 0.0, false);
        // Far out on X and Y, but below the plane on Z: kept.
        assert!(plane.keeps([1e4, -1e4, -1.0]));
        assert!(!plane.keeps([1e4, -1e4, 1.0]));
    }

    #[test]
    fn a_point_on_the_plane_is_kept_either_way_round() {
        let point = [1.5, 0.0, 0.0];
        assert!(section(Axis::X, 1.5, false).keeps(point));
        assert!(section(Axis::X, 1.5, true).keeps(point));
    }

    fn bounds(min: [f32; 3], max: [f32; 3]) -> Bounds3 {
        Bounds3 { min, max }
    }

    #[test]
    fn the_plane_travels_across_the_model_and_a_little_past_each_end() {
        let model = bounds([-2.0, 0.0, 1.0], [8.0, 4.0, 1.0]);
        let (low, high) = travel_along(Some(model), Axis::X);
        assert!(
            low < -2.0 && high > 8.0,
            "travel {low}..{high} clips the model"
        );
        // A plane parked at either extreme must leave the whole model on one
        // side, or the ends of the slider do nothing visible.
        let ends = SectionPlane {
            enabled: true,
            axis: Axis::X,
            offset: low,
            flipped: false,
        };
        assert!(!ends.keeps([-2.0, 0.0, 1.0]));
        let other = SectionPlane {
            offset: high,
            ..ends
        };
        assert!(other.keeps([8.0, 0.0, 1.0]));
    }

    #[test]
    fn an_axis_the_model_is_flat_on_still_gives_the_plane_room_to_move() {
        // Z is a single value here; a zero-width range would freeze the
        // control and the section would never move.
        let (low, high) = travel_along(Some(bounds([-2.0, 0.0, 1.0], [8.0, 4.0, 1.0])), Axis::Z);
        assert!(high > low, "flat axis gave an empty range {low}..{high}");
        assert!(low < 1.0 && high > 1.0);
    }

    #[test]
    fn changing_axis_re_centres_the_plane_on_the_model() {
        let model = bounds([-2.0, 100.0, 0.0], [8.0, 140.0, 0.0]);
        let mut plane = section(Axis::X, 7.0, false);
        plane.cut_along(Axis::Y, Some(model));

        assert_eq!(plane.axis, Axis::Y);
        // Carrying the old offset of 7.0 onto Y would put the plane far
        // below a model that lives between 100 and 140: no visible cut.
        assert!(
            plane.keeps([0.0, 110.0, 0.0]) && !plane.keeps([0.0, 130.0, 0.0]),
            "the plane at {} does not cut the model",
            plane.offset
        );
    }

    #[test]
    fn the_equation_the_shaders_bind_decides_the_same_side_as_the_predicate() {
        // The shaders never call `keeps`; they evaluate `equation`. If the
        // two disagree the viewport clips the opposite half from everything
        // tested here.
        for axis in Axis::ALL {
            for flipped in [false, true] {
                for offset in [-3.0, 0.0, 7.25f32] {
                    let plane = section(axis, offset, flipped);
                    let equation = plane.equation();
                    for coordinate in [-9.0, -3.0, 0.0, 1.0, 7.25, 12.0f32] {
                        for other in [-5.0, 0.0, 5.0f32] {
                            let mut point = [other; 3];
                            point[axis.index()] = coordinate;
                            assert_eq!(
                                plane.keeps(point),
                                shader_keeps(equation, point),
                                "{plane:?} disagreed with its own equation at {point:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
