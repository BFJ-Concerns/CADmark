// A sketch profile as the renderer consumes it: the plane the profile was
// drawn on, its curves as polylines, its corners as points, and its
// enclosed regions as triangulated areas. Kernel-neutral plain data,
// serialisable so it can cross the kernel worker's process boundary.
//
// This is the only point geometry in CADmark: `TessellatedMesh` carries no
// vertex positions, so a sketch's corners are drawn from here rather than
// from the solid mesh.

use serde::{Deserialize, Serialize};

/// The plane a sketch was drawn on, as an origin and an orthonormal frame.
/// `normal` is what a flat profile view looks down; `x_axis` fixes the
/// in-plane rotation so the same sketch always frames the same way up.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SketchPlane {
    pub origin: [f32; 3],
    pub normal: [f32; 3],
    pub x_axis: [f32; 3],
}

impl Default for SketchPlane {
    fn default() -> Self {
        Self {
            origin: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            x_axis: [1.0, 0.0, 0.0],
        }
    }
}

/// One curve of a sketch, sampled densely enough to draw as a polyline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchCurve {
    /// Zero-based index of the curve's edge in the sketch's own topology.
    pub curve_id: u32,
    /// Polyline vertices approximating the curve, in order along it.
    pub points: Vec<[f32; 3]>,
}

/// One corner of a sketch: where two curves meet, or a curve ends.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SketchCorner {
    /// Zero-based index of the corner's vertex in the sketch's own topology.
    pub corner_id: u32,
    pub position: [f32; 3],
}

/// One enclosed region of a sketch, triangulated so it can be filled. A
/// region with a hole in it is one region whose triangulation omits the
/// hole, not two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchRegion {
    /// Zero-based index of the region's face in the sketch's own topology.
    pub region_id: u32,
    pub vertices: Vec<[f32; 3]>,
    /// Triangle indices into `vertices` — every three consecutive form a
    /// triangle.
    pub indices: Vec<u32>,
}

/// Everything drawn by a script that has reached a sketch and no solid.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SketchProfile {
    pub plane: SketchPlane,
    pub curves: Vec<SketchCurve>,
    pub corners: Vec<SketchCorner>,
    pub regions: Vec<SketchRegion>,
}

impl SketchProfile {
    /// Every point the profile occupies, whatever kind of element it
    /// belongs to — what a camera frames and a bounding box is taken over.
    pub fn points(&self) -> impl Iterator<Item = [f32; 3]> + '_ {
        self.curves
            .iter()
            .flat_map(|curve| curve.points.iter().copied())
            .chain(self.corners.iter().map(|corner| corner.position))
            .chain(
                self.regions
                    .iter()
                    .flat_map(|region| region.vertices.iter().copied()),
            )
    }

    /// The widest span of the profile across any axis, or zero when it has
    /// no extent. Sizes anything drawn in proportion to the sketch.
    pub fn extent(&self) -> f32 {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for point in self.points() {
            for axis in 0..3 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        }
        (0..3)
            .map(|axis| max[axis] - min[axis])
            .fold(0.0f32, |widest, span| {
                if span.is_finite() {
                    widest.max(span)
                } else {
                    widest
                }
            })
    }

    /// What the status line and the AI read: a sketch counted by the kinds
    /// of element it drew, never as a solid's measurements.
    pub fn describe(&self) -> String {
        fn plural(count: usize, singular: &str, plural: &str) -> String {
            format!("{count} {}", if count == 1 { singular } else { plural })
        }
        format!(
            "A sketch profile, not yet a solid: {}, {}, {}",
            plural(self.curves.len(), "curve", "curves"),
            plural(self.corners.len(), "corner", "corners"),
            plural(self.regions.len(), "enclosed region", "enclosed regions"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> SketchProfile {
        let corners = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 3.0, 0.0],
            [0.0, 3.0, 0.0],
        ];
        SketchProfile {
            plane: SketchPlane::default(),
            curves: (0..4)
                .map(|index| SketchCurve {
                    curve_id: index as u32,
                    points: vec![corners[index], corners[(index + 1) % 4]],
                })
                .collect(),
            corners: corners
                .iter()
                .enumerate()
                .map(|(index, position)| SketchCorner {
                    corner_id: index as u32,
                    position: *position,
                })
                .collect(),
            regions: vec![SketchRegion {
                region_id: 0,
                vertices: corners.to_vec(),
                indices: vec![0, 1, 2, 0, 2, 3],
            }],
        }
    }

    #[test]
    fn a_profiles_extent_is_its_widest_span_across_every_element() {
        assert_eq!(square().extent(), 3.0);
    }

    #[test]
    fn an_empty_profile_has_no_extent_rather_than_an_infinite_one() {
        assert_eq!(SketchProfile::default().extent(), 0.0);
    }

    #[test]
    fn a_profile_describes_itself_by_what_it_drew_not_as_a_solid() {
        assert_eq!(
            square().describe(),
            "A sketch profile, not yet a solid: 4 curves, 4 corners, 1 enclosed region"
        );
        assert_eq!(
            SketchProfile::default().describe(),
            "A sketch profile, not yet a solid: 0 curves, 0 corners, 0 enclosed regions"
        );
    }

    #[test]
    fn a_profile_round_trips_through_json_as_the_boundary_carries_it() {
        let profile = square();
        let line = serde_json::to_string(&profile).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(
            serde_json::from_str::<SketchProfile>(&line).unwrap(),
            profile
        );
    }
}
