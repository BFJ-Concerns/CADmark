// How wide an edge is drawn and how large a vertex marker is — in screen
// pixels, so both hold their size at any camera distance.
//
// One `MarkerSizing` feeds every pass. The visible passes and the picking
// passes read their widths from `MarkerExtent`, which is a field of both
// uniform structs and is produced only by `MarkerSizing::extent`: a pass
// drawn wider than it picks, or picking wider than it draws, would need
// the shared value to be split first.

use bytemuck::{Pod, Zeroable};

/// Marker sizes in logical screen pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarkerSizing {
    /// Half the width of a drawn edge, so an edge is twice this wide.
    pub edge_half_width_px: f32,
    /// Radius of a vertex marker's disc.
    pub vertex_radius_px: f32,
}

impl Default for MarkerSizing {
    fn default() -> Self {
        // A six-pixel edge and an eleven-pixel vertex disc: wide enough to
        // aim at with a mouse without burying small features.
        Self {
            edge_half_width_px: 3.0,
            vertex_radius_px: 5.5,
        }
    }
}

impl MarkerSizing {
    /// The same sizes as clip-space half-extents for a viewport of
    /// `width` by `height` physical pixels. Normalised device coordinates
    /// span 2 across the viewport, so one pixel is `2 / size` of it.
    pub fn extent(self, width: u32, height: u32) -> MarkerExtent {
        let w = width.max(1) as f32;
        let h = height.max(1) as f32;
        MarkerExtent {
            edge_half_width_ndc: [
                2.0 * self.edge_half_width_px / w,
                2.0 * self.edge_half_width_px / h,
            ],
            vertex_radius_ndc: [
                2.0 * self.vertex_radius_px / w,
                2.0 * self.vertex_radius_px / h,
            ],
        }
    }
}

/// Marker half-extents in clip space, as the shaders consume them.
/// Mirrored in the WGSL `MarkerExtent` struct of `markers_common.wgsl`,
/// which every shader binding a uniform block including it shares.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct MarkerExtent {
    pub edge_half_width_ndc: [f32; 2],
    pub vertex_radius_ndc: [f32; 2],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_size_becomes_its_share_of_clip_space() {
        let sizing = MarkerSizing {
            edge_half_width_px: 4.0,
            vertex_radius_px: 10.0,
        };
        let extent = sizing.extent(800, 400);
        // Clip space spans 2.0, so 4 pixels of 800 is 4 * 2 / 800.
        assert_eq!(extent.edge_half_width_ndc, [0.01, 0.02]);
        assert_eq!(extent.vertex_radius_ndc, [0.025, 0.05]);
    }

    #[test]
    fn a_marker_keeps_its_pixel_size_as_the_viewport_grows() {
        let sizing = MarkerSizing::default();
        for (w, h) in [(320u32, 240u32), (1920, 1080), (3840, 2160)] {
            let extent = sizing.extent(w, h);
            let width_px = extent.edge_half_width_ndc[0] * w as f32 / 2.0;
            assert!((width_px - sizing.edge_half_width_px).abs() < 1e-4);
        }
    }

    #[test]
    fn a_zero_sized_viewport_produces_finite_extents() {
        let extent = MarkerSizing::default().extent(0, 0);
        assert!(extent.edge_half_width_ndc.iter().all(|v| v.is_finite()));
        assert!(extent.vertex_radius_ndc.iter().all(|v| v.is_finite()));
    }
}
