// GPU colour-ID picking — offscreen render pass that encodes each
// topological element as a unique colour.
//
// Faces, edges, and vertices occupy distinct ID ranges so the type
// and index can be recovered from a single pixel readback.

use cadmark_core::geometry::{
    EdgeId, FaceId, PartId, PickedElement, SketchElement, SketchElementKind, TopologyElement,
    VertexId,
};

/// ID ranges for each element type in the picking pass.
/// Face IDs:  1..EDGE_OFFSET-1
/// Edge IDs:  EDGE_OFFSET..VERTEX_OFFSET-1
/// Vertex IDs: VERTEX_OFFSET..PART_OFFSET-1
/// Part IDs: PART_OFFSET..SKETCH_CURVE_OFFSET-1
/// Sketch curve IDs: SKETCH_CURVE_OFFSET..SKETCH_CORNER_OFFSET-1
/// Sketch corner IDs: SKETCH_CORNER_OFFSET..SKETCH_REGION_OFFSET-1
/// Sketch region IDs: SKETCH_REGION_OFFSET..
///
/// ID 0 = background (no element).
const EDGE_OFFSET: u32 = 100_000;
const VERTEX_OFFSET: u32 = 200_000;
const PART_OFFSET: u32 = 300_000;
const SKETCH_CURVE_OFFSET: u32 = 400_000;
const SKETCH_CORNER_OFFSET: u32 = 500_000;
const SKETCH_REGION_OFFSET: u32 = 600_000;

/// Which kinds of element a click may land on. Every kind is enabled
/// until the user turns one off; a disabled kind is not drawn into the
/// colour-ID texture at all, so a click where it would have been reaches
/// whatever is behind it rather than reading as empty space.
///
/// The filter governs the three topology kinds the user aims at within a
/// part. A whole-part pick is a different question — which part, not
/// which element of it — so it passes through untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionFilter {
    pub faces: bool,
    pub edges: bool,
    pub vertices: bool,
}

impl Default for SelectionFilter {
    fn default() -> Self {
        Self {
            faces: true,
            edges: true,
            vertices: true,
        }
    }
}

impl SelectionFilter {
    /// Whether a readback may resolve to this element.
    pub fn allows(&self, element: &TopologyElement) -> bool {
        match element {
            TopologyElement::Face(_) => self.faces,
            TopologyElement::Edge(_) => self.edges,
            TopologyElement::Vertex(_) => self.vertices,
            TopologyElement::Part(_) => true,
        }
    }
}

/// Encode a topology element as a picking ID for the colour buffer.
pub fn encode_picking_id(element: &TopologyElement) -> u32 {
    match element {
        TopologyElement::Part(PartId(id)) => PART_OFFSET + *id + 1,
        TopologyElement::Face(FaceId(id)) => *id + 1,
        TopologyElement::Edge(EdgeId(id)) => EDGE_OFFSET + *id + 1,
        TopologyElement::Vertex(VertexId(id)) => VERTEX_OFFSET + *id + 1,
    }
}

/// Decode a pixel value from the picking buffer into a topology element.
/// Returns None for background (0) or out-of-range values.
pub fn decode_picking_id(id: u32) -> Option<TopologyElement> {
    if id == 0 {
        None
    } else if id < EDGE_OFFSET {
        Some(TopologyElement::Face(FaceId(id - 1)))
    } else if id < VERTEX_OFFSET {
        Some(TopologyElement::Edge(EdgeId(id - EDGE_OFFSET - 1)))
    } else if id < PART_OFFSET {
        Some(TopologyElement::Vertex(VertexId(id - VERTEX_OFFSET - 1)))
    } else if id == PART_OFFSET {
        None
    } else if id < SKETCH_CURVE_OFFSET {
        Some(TopologyElement::Part(PartId(id - PART_OFFSET - 1)))
    } else {
        None
    }
}

/// Encode anything the user can click — solid topology or a drawn sketch
/// element — as a picking ID.
pub fn encode_pick(element: &PickedElement) -> u32 {
    match element {
        PickedElement::Solid(element) => encode_picking_id(element),
        PickedElement::Sketch(SketchElement { kind, index }) => match kind {
            SketchElementKind::Curve => SKETCH_CURVE_OFFSET + *index + 1,
            SketchElementKind::Corner => SKETCH_CORNER_OFFSET + *index + 1,
            SketchElementKind::Region => SKETCH_REGION_OFFSET + *index + 1,
        },
    }
}

/// Decode any pickable element, including sketch IDs outside the solid
/// topology ranges.
pub fn decode_pick(id: u32) -> Option<PickedElement> {
    if let Some(element) = decode_picking_id(id) {
        return Some(PickedElement::Solid(element));
    }
    let (kind, offset) = if id > SKETCH_REGION_OFFSET {
        (SketchElementKind::Region, SKETCH_REGION_OFFSET)
    } else if id > SKETCH_CORNER_OFFSET {
        (SketchElementKind::Corner, SKETCH_CORNER_OFFSET)
    } else if id > SKETCH_CURVE_OFFSET {
        (SketchElementKind::Curve, SKETCH_CURVE_OFFSET)
    } else {
        return None;
    };
    Some(PickedElement::Sketch(SketchElement {
        kind,
        index: id - offset - 1,
    }))
}

/// Encode a picking ID as RGBA bytes for the colour attachment.
/// Splits the 32-bit ID across all four channels in little-endian order.
pub fn id_to_colour(id: u32) -> [u8; 4] {
    [
        (id & 0xFF) as u8,
        ((id >> 8) & 0xFF) as u8,
        ((id >> 16) & 0xFF) as u8,
        ((id >> 24) & 0xFF) as u8,
    ]
}

/// Decode RGBA pixel bytes back to a picking ID.
pub fn colour_to_id(pixel: [u8; 4]) -> u32 {
    (pixel[0] as u32)
        | ((pixel[1] as u32) << 8)
        | ((pixel[2] as u32) << 16)
        | ((pixel[3] as u32) << 24)
}

/// State for the picking readback — manages the offscreen texture and
/// staging buffer for async GPU readback.
pub struct PickingPass {
    pub texture: wgpu::Texture,
    pub texture_view: wgpu::TextureView,
    pub staging_buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
}

impl PickingPass {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("picking_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Uint,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Staging buffer for reading back a single pixel.
        // Aligned to 256 bytes as required by wgpu.
        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("picking_staging"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            texture,
            texture_view,
            staging_buffer,
            width,
            height,
        }
    }

    /// Resize the picking texture when the viewport changes.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        *self = Self::new(device, width, height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_id_roundtrip() {
        let face = TopologyElement::Face(FaceId(42));
        let id = encode_picking_id(&face);
        let decoded = decode_picking_id(id);
        assert_eq!(decoded, Some(face));
    }

    #[test]
    fn edge_id_roundtrip() {
        let edge = TopologyElement::Edge(EdgeId(7));
        let id = encode_picking_id(&edge);
        let decoded = decode_picking_id(id);
        assert_eq!(decoded, Some(edge));
    }

    #[test]
    fn vertex_id_roundtrip() {
        let vertex = TopologyElement::Vertex(VertexId(0));
        let id = encode_picking_id(&vertex);
        let decoded = decode_picking_id(id);
        assert_eq!(decoded, Some(vertex));
    }

    #[test]
    fn part_ids_use_their_own_range_at_every_boundary() {
        assert_eq!(
            decode_picking_id(PART_OFFSET - 1),
            Some(TopologyElement::Vertex(VertexId(
                PART_OFFSET - VERTEX_OFFSET - 2
            )))
        );
        assert_eq!(decode_picking_id(PART_OFFSET), None);
        let part = TopologyElement::Part(PartId(0));
        assert_eq!(encode_picking_id(&part), PART_OFFSET + 1);
        assert_eq!(decode_picking_id(PART_OFFSET + 1), Some(part));
    }

    #[test]
    fn background_decodes_to_none() {
        assert_eq!(decode_picking_id(0), None);
    }

    #[test]
    fn the_filter_enables_every_kind_by_default() {
        let filter = SelectionFilter::default();
        assert!(filter.allows(&TopologyElement::Face(FaceId(0))));
        assert!(filter.allows(&TopologyElement::Edge(EdgeId(0))));
        assert!(filter.allows(&TopologyElement::Vertex(VertexId(0))));
    }

    #[test]
    fn a_disabled_kind_is_the_only_kind_the_filter_refuses() {
        // Each arm is independent, so each one is turned off in turn: a
        // filter that refuses the wrong kind, or refuses nothing, shows here.
        let elements = [
            TopologyElement::Face(FaceId(9)),
            TopologyElement::Edge(EdgeId(9)),
            TopologyElement::Vertex(VertexId(9)),
        ];
        let disabled = [
            SelectionFilter {
                faces: false,
                ..SelectionFilter::default()
            },
            SelectionFilter {
                edges: false,
                ..SelectionFilter::default()
            },
            SelectionFilter {
                vertices: false,
                ..SelectionFilter::default()
            },
        ];
        for (off, filter) in disabled.iter().enumerate() {
            for (kind, element) in elements.iter().enumerate() {
                assert_eq!(
                    filter.allows(element),
                    kind != off,
                    "{filter:?} judged {element:?} wrongly"
                );
            }
        }
    }

    #[test]
    fn a_whole_part_pick_is_not_a_kind_the_filter_can_turn_off() {
        // Picking a part answers "which part", not "which element of it".
        // Turning every topology kind off must still leave a part pickable.
        let nothing = SelectionFilter {
            faces: false,
            edges: false,
            vertices: false,
        };
        assert!(nothing.allows(&TopologyElement::Part(PartId(3))));
    }

    #[test]
    fn sketch_elements_roundtrip_in_their_own_ranges() {
        for kind in [
            SketchElementKind::Curve,
            SketchElementKind::Corner,
            SketchElementKind::Region,
        ] {
            for index in [0, 1, 99_998] {
                let element = PickedElement::Sketch(SketchElement { kind, index });
                assert_eq!(decode_pick(encode_pick(&element)), Some(element));
            }
        }
    }

    #[test]
    fn solid_and_sketch_ranges_never_collide() {
        let solid = [
            TopologyElement::Face(FaceId(3)),
            TopologyElement::Edge(EdgeId(3)),
            TopologyElement::Vertex(VertexId(3)),
            TopologyElement::Part(PartId(3)),
        ];
        for element in solid {
            let id = encode_picking_id(&element);
            assert_eq!(
                decode_pick(id),
                Some(PickedElement::Solid(element.clone())),
                "solid IDs still decode as solid topology",
            );
        }
        // A sketch pixel is not a very high part index: the solid-only
        // decoder must decline it.
        let sketch = encode_pick(&PickedElement::Sketch(SketchElement {
            kind: SketchElementKind::Curve,
            index: 0,
        }));
        assert_eq!(decode_picking_id(sketch), None);
    }

    #[test]
    fn colour_encoding_roundtrip() {
        let id = 0x00_AB_CD_EF;
        let colour = id_to_colour(id);
        let decoded = colour_to_id(colour);
        assert_eq!(decoded, id);
    }
}
