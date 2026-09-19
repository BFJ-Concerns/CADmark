// A sketch profile as the renderer consumes it: the plane the profile was
// drawn on, its curves as polylines, its corners as points, and its
// enclosed regions as triangulated areas. Kernel-neutral plain data,
// serialisable so it can cross the kernel worker's process boundary.
//
// This is the only point geometry in CADmark: `TessellatedMesh` carries no
// vertex positions, so a sketch's corners are drawn from here rather than
// from the solid mesh.
//
// Each element also carries what the kernel measured of it — a curve's
// type and length, a region's area — so the status bar and the AI can read
// a sketch's dimensions as they read a solid's.

use serde::{Deserialize, Serialize};

use crate::geometry::{SketchElement, SketchElementKind, compact};

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

impl SketchPlane {
    /// The in-plane vertical axis: the normal crossed with the x axis, so
    /// `(x_axis, y_axis, normal)` is right-handed.
    pub fn y_axis(&self) -> [f32; 3] {
        let (n, x) = (self.normal, self.x_axis);
        [
            n[1] * x[2] - n[2] * x[1],
            n[2] * x[0] - n[0] * x[2],
            n[0] * x[1] - n[1] * x[0],
        ]
    }
}

/// One curve of a sketch, sampled densely enough to draw as a polyline.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SketchCurve {
    /// Zero-based index of the curve's edge in the sketch's own topology.
    pub curve_id: u32,
    /// Polyline vertices approximating the curve, in order along it.
    pub points: Vec<[f32; 3]>,
    /// OCCT curve classification, e.g. "line", "circle", "bspline".
    pub curve_type: String,
    /// Exact arc length in mm, from the curve rather than the polyline.
    pub length: f64,
    /// Circle radius when this is a circular curve; absent otherwise.
    pub radius: Option<f64>,
}

/// One corner of a sketch: where two curves meet, or a curve ends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct SketchCorner {
    /// Zero-based index of the corner's vertex in the sketch's own topology.
    pub corner_id: u32,
    pub position: [f32; 3],
}

/// One enclosed region of a sketch, triangulated so it can be filled. A
/// region with a hole in it is one region whose triangulation omits the
/// hole, not two.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SketchRegion {
    /// Zero-based index of the region's face in the sketch's own topology.
    pub region_id: u32,
    pub vertices: Vec<[f32; 3]>,
    /// Triangle indices into `vertices` — every three consecutive form a
    /// triangle.
    pub indices: Vec<u32>,
    /// Exact enclosed area in mm², holes excluded.
    pub area: f64,
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

    /// The profile's overall size in its own plane — width along the
    /// plane's x axis and height along its y axis — or zeros when it has
    /// no extent. What a designer reads as the sketch's dimensions.
    pub fn size(&self) -> [f32; 2] {
        let axes = [self.plane.x_axis, self.plane.y_axis()];
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for point in self.points() {
            for (index, axis) in axes.iter().enumerate() {
                let along = (0..3).map(|c| point[c] * axis[c]).sum::<f32>();
                min[index] = min[index].min(along);
                max[index] = max[index].max(along);
            }
        }
        std::array::from_fn(|index| {
            let span = max[index] - min[index];
            if span.is_finite() { span } else { 0.0 }
        })
    }

    /// What the status line and the AI read: a sketch counted by the kinds
    /// of element it drew and sized in its plane, never as a solid's
    /// measurements.
    pub fn describe(&self) -> String {
        fn plural(count: usize, singular: &str, plural: &str) -> String {
            format!("{count} {}", if count == 1 { singular } else { plural })
        }
        let size = self.size();
        format!(
            "A sketch profile, not yet a solid: {} × {} mm, {}, {}, {}",
            compact(f64::from(size[0])),
            compact(f64::from(size[1])),
            plural(self.curves.len(), "curve", "curves"),
            plural(self.corners.len(), "corner", "corners"),
            plural(self.regions.len(), "enclosed region", "enclosed regions"),
        )
    }

    /// The measurement of one element, as the status bar shows it beside
    /// the selection: a curve's length or a circular curve's diameter, a
    /// region's area. A corner has only a position, which is not a
    /// measurement.
    pub fn measurement(&self, element: &SketchElement) -> Option<String> {
        match element.kind {
            SketchElementKind::Curve => self.curve(element.index).map(|curve| {
                if let Some(radius) = curve.radius {
                    format!("Diameter {} mm", compact(radius * 2.0))
                } else {
                    format!("Length {} mm", compact(curve.length))
                }
            }),
            SketchElementKind::Region => self
                .region(element.index)
                .map(|region| format!("Area {} mm²", compact(region.area))),
            SketchElementKind::Corner => None,
        }
    }

    /// What the AI is told about one element's measured geometry, as the
    /// key-value pairs the solid path uses for faces, edges and vertices.
    pub fn identification(
        &self,
        element: &SketchElement,
    ) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        match element.kind {
            SketchElementKind::Curve => {
                if let Some(curve) = self.curve(element.index) {
                    map.insert("curve".into(), curve.curve_type.clone());
                    map.insert("length_mm".into(), format!("{:.2}", curve.length));
                    if let Some(radius) = curve.radius {
                        map.insert("radius_mm".into(), format!("{radius:.2}"));
                    }
                    if let Some(centre) = centroid(&curve.points) {
                        map.insert("centre_mm".into(), triple(centre));
                    }
                }
            }
            SketchElementKind::Corner => {
                if let Some(corner) = self.corner(element.index) {
                    map.insert("position_mm".into(), triple(corner.position));
                }
            }
            SketchElementKind::Region => {
                if let Some(region) = self.region(element.index) {
                    map.insert("area_mm2".into(), format!("{:.2}", region.area));
                    if let Some(centre) = centroid(&region.vertices) {
                        map.insert("centre_mm".into(), triple(centre));
                    }
                    map.insert("plane_normal".into(), triple(self.plane.normal));
                }
            }
        }
        map
    }

    pub fn curve(&self, id: u32) -> Option<&SketchCurve> {
        self.curves.iter().find(|curve| curve.curve_id == id)
    }

    pub fn corner(&self, id: u32) -> Option<&SketchCorner> {
        self.corners.iter().find(|corner| corner.corner_id == id)
    }

    pub fn region(&self, id: u32) -> Option<&SketchRegion> {
        self.regions.iter().find(|region| region.region_id == id)
    }
}

fn centroid(points: &[[f32; 3]]) -> Option<[f32; 3]> {
    if points.is_empty() {
        return None;
    }
    let count = points.len() as f32;
    Some(std::array::from_fn(|axis| {
        points.iter().map(|point| point[axis]).sum::<f32>() / count
    }))
}

fn triple(values: [f32; 3]) -> String {
    format!("({:.2}, {:.2}, {:.2})", values[0], values[1], values[2])
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
                    curve_type: "line".to_string(),
                    length: if index % 2 == 0 { 2.0 } else { 3.0 },
                    radius: None,
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
                area: 6.0,
            }],
        }
    }

    #[test]
    fn a_profiles_extent_is_its_widest_span_across_every_element() {
        assert_eq!(square().extent(), 3.0);
    }

    #[test]
    fn a_profiles_size_is_measured_in_its_own_plane() {
        assert_eq!(square().size(), [2.0, 3.0]);
        // The same rectangle stood up on the YZ plane is still 2 wide and
        // 3 high in that plane, whatever its world extents are.
        let mut standing = square();
        standing.plane = SketchPlane {
            origin: [0.0; 3],
            normal: [1.0, 0.0, 0.0],
            x_axis: [0.0, 1.0, 0.0],
        };
        for point in standing
            .curves
            .iter_mut()
            .flat_map(|curve| curve.points.iter_mut())
            .chain(
                standing
                    .corners
                    .iter_mut()
                    .map(|corner| &mut corner.position),
            )
            .chain(
                standing
                    .regions
                    .iter_mut()
                    .flat_map(|region| region.vertices.iter_mut()),
            )
        {
            *point = [0.0, point[0], point[1]];
        }
        assert_eq!(standing.size(), [2.0, 3.0]);
    }

    #[test]
    fn an_empty_profile_has_no_extent_rather_than_an_infinite_one() {
        assert_eq!(SketchProfile::default().extent(), 0.0);
        assert_eq!(SketchProfile::default().size(), [0.0, 0.0]);
    }

    #[test]
    fn a_profile_describes_itself_by_what_it_drew_not_as_a_solid() {
        assert_eq!(
            square().describe(),
            "A sketch profile, not yet a solid: 2 × 3 mm, 4 curves, 4 corners, 1 enclosed region"
        );
        assert_eq!(
            SketchProfile::default().describe(),
            "A sketch profile, not yet a solid: 0 × 0 mm, 0 curves, 0 corners, 0 enclosed regions"
        );
    }

    #[test]
    fn each_element_is_measured_as_the_status_bar_and_the_ai_read_it() {
        let mut profile = square();
        profile.curves.push(SketchCurve {
            curve_id: 9,
            points: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
            curve_type: "circle".to_string(),
            length: std::f64::consts::TAU,
            radius: Some(1.0),
        });
        let curve = SketchElement {
            kind: SketchElementKind::Curve,
            index: 1,
        };
        let circle = SketchElement {
            kind: SketchElementKind::Curve,
            index: 9,
        };
        let region = SketchElement {
            kind: SketchElementKind::Region,
            index: 0,
        };
        let corner = SketchElement {
            kind: SketchElementKind::Corner,
            index: 2,
        };
        let missing = SketchElement {
            kind: SketchElementKind::Region,
            index: 4,
        };

        assert_eq!(profile.measurement(&curve).as_deref(), Some("Length 3 mm"));
        assert_eq!(
            profile.measurement(&circle).as_deref(),
            Some("Diameter 2 mm")
        );
        assert_eq!(profile.measurement(&region).as_deref(), Some("Area 6 mm²"));
        assert_eq!(profile.measurement(&corner), None);
        assert_eq!(profile.measurement(&missing), None);

        let identified = profile.identification(&curve);
        assert_eq!(identified.get("curve").map(String::as_str), Some("line"));
        assert_eq!(
            identified.get("length_mm").map(String::as_str),
            Some("3.00")
        );
        assert_eq!(
            profile
                .identification(&circle)
                .get("radius_mm")
                .map(String::as_str),
            Some("1.00")
        );
        assert_eq!(
            profile
                .identification(&corner)
                .get("position_mm")
                .map(String::as_str),
            Some("(2.00, 3.00, 0.00)")
        );
        let region_identified = profile.identification(&region);
        assert_eq!(
            region_identified.get("area_mm2").map(String::as_str),
            Some("6.00")
        );
        assert_eq!(
            region_identified.get("plane_normal").map(String::as_str),
            Some("(0.00, 0.00, 1.00)")
        );
        assert!(profile.identification(&missing).is_empty());
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
