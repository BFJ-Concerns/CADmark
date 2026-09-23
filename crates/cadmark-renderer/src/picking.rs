// GPU colour-ID picking — offscreen render pass that encodes each
// topological element as a unique colour.
//
// Faces, edges, and vertices occupy distinct ID ranges so the type
// and index can be recovered from a single pixel readback. Those ranges
// number elements within one part; the part itself sits in the ID's
// high bits, so a scene of several parts reads back as one pixel too.

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
/// ID 0 = background (no element). Every range ends below bit
/// `PART_SHIFT`, where the part ordinal begins.
const EDGE_OFFSET: u32 = 100_000;
const VERTEX_OFFSET: u32 = 200_000;
/// Start of the whole-part range: the shaders derive a part's ordinal from
/// the part ID its vertices carry by subtracting this and one.
pub const PART_OFFSET: u32 = 300_000;
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

    /// Whether a readback may resolve to this pick, of either kind. A
    /// sketch's regions, curves and corners follow the face, edge and
    /// vertex toggles: they are the same three shapes of target.
    pub fn allows_pick(&self, picked: &Pick) -> bool {
        match picked {
            Pick::Solid { element, .. } => self.allows(element),
            Pick::Sketch(SketchElement { kind, .. }) => match kind {
                SketchElementKind::Region => self.faces,
                SketchElementKind::Curve => self.edges,
                SketchElementKind::Corner => self.vertices,
            },
        }
    }
}

/// Bit position of the part ordinal within a solid pick ID. The element
/// ranges above stay below this bit, so the low bits of an ID name an
/// element within a part and the high bits name the part.
pub const PART_SHIFT: u32 = 20;
const ELEMENT_MASK: u32 = (1 << PART_SHIFT) - 1;

/// Anything a pixel of the ID texture can name: an element of one part,
/// numbered within that part, or a sketch element, which belongs to no
/// part. Face, edge and vertex IDs restart in every part, so a solid pick
/// is only meaningful with the part it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Solid {
        part: PartId,
        element: TopologyElement,
    },
    Sketch(SketchElement),
}

impl Pick {
    /// The whole of one part.
    pub fn part(part: PartId) -> Self {
        Self::Solid {
            part,
            element: TopologyElement::Part(part),
        }
    }

    /// The element this pick names, without its part.
    pub fn element(&self) -> PickedElement {
        match self {
            Self::Solid { element, .. } => PickedElement::Solid(element.clone()),
            Self::Sketch(element) => PickedElement::Sketch(*element),
        }
    }

    /// The part a solid pick is numbered within; a sketch pick has none.
    pub fn part_id(&self) -> Option<PartId> {
        match self {
            Self::Solid { part, .. } => Some(*part),
            Self::Sketch(_) => None,
        }
    }
}

/// Encode a topology element as a picking ID within its own part: the
/// low bits, before the part ordinal is added. This is what the vertex
/// buffers carry; the shaders add the part.
pub fn encode_picking_id(element: &TopologyElement) -> u32 {
    match element {
        TopologyElement::Part(PartId(id)) => PART_OFFSET + *id + 1,
        TopologyElement::Face(FaceId(id)) => *id + 1,
        TopologyElement::Edge(EdgeId(id)) => EDGE_OFFSET + *id + 1,
        TopologyElement::Vertex(VertexId(id)) => VERTEX_OFFSET + *id + 1,
    }
}

/// Decode the within-part bits of a pick ID into a topology element.
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

/// Encode anything the user can click as the ID the colour buffer holds
/// and the highlight uniforms compare against: a solid element within
/// its part, or a drawn sketch element.
pub fn encode_pick(pick: &Pick) -> u32 {
    match pick {
        Pick::Solid { part, element } => encode_picking_id(element) | (part.0 << PART_SHIFT),
        Pick::Sketch(SketchElement { kind, index }) => match kind {
            SketchElementKind::Curve => SKETCH_CURVE_OFFSET + *index + 1,
            SketchElementKind::Corner => SKETCH_CORNER_OFFSET + *index + 1,
            SketchElementKind::Region => SKETCH_REGION_OFFSET + *index + 1,
        },
    }
}

/// Decode a pixel's ID into what it names, or None for the background.
pub fn decode_pick(id: u32) -> Option<Pick> {
    let element = id & ELEMENT_MASK;
    if let Some(element) = decode_picking_id(element) {
        return Some(Pick::Solid {
            part: PartId(id >> PART_SHIFT),
            element,
        });
    }
    let (kind, offset) = if element > SKETCH_REGION_OFFSET {
        (SketchElementKind::Region, SKETCH_REGION_OFFSET)
    } else if element > SKETCH_CORNER_OFFSET {
        (SketchElementKind::Corner, SKETCH_CORNER_OFFSET)
    } else if element > SKETCH_CURVE_OFFSET {
        (SketchElementKind::Curve, SKETCH_CURVE_OFFSET)
    } else {
        return None;
    };
    Some(Pick::Sketch(SketchElement {
        kind,
        index: element - offset - 1,
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
                let pick = Pick::Sketch(SketchElement { kind, index });
                assert_eq!(decode_pick(encode_pick(&pick)), Some(pick));
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
            let part = PartId(3);
            let id = encode_pick(&Pick::Solid {
                part,
                element: element.clone(),
            });
            assert_eq!(
                decode_pick(id),
                Some(Pick::Solid { part, element }),
                "solid IDs still decode as solid topology",
            );
        }
        // A sketch pixel is not a very high part index: the solid-only
        // decoder must decline it.
        let sketch = encode_pick(&Pick::Sketch(SketchElement {
            kind: SketchElementKind::Curve,
            index: 0,
        }));
        assert_eq!(decode_picking_id(sketch), None);
        assert_eq!(decode_pick(sketch).and_then(|pick| pick.part_id()), None);
    }

    #[test]
    fn a_solid_pick_carries_its_part_and_survives_the_round_trip() {
        // Face, edge and vertex numbering restarts in every part, so the
        // same local element in two parts must give two different IDs, and
        // each must decode back to the part it was drawn for.
        let elements = [
            TopologyElement::Face(FaceId(7)),
            TopologyElement::Edge(EdgeId(7)),
            TopologyElement::Vertex(VertexId(7)),
        ];
        for part in [0, 1, 2, 4095] {
            for element in &elements {
                let pick = Pick::Solid {
                    part: PartId(part),
                    element: element.clone(),
                };
                let id = encode_pick(&pick);
                assert_eq!(decode_pick(id), Some(pick), "part {part} {element:?}");
                let elsewhere = encode_pick(&Pick::Solid {
                    part: PartId(part + 1),
                    element: element.clone(),
                });
                assert_ne!(id, elsewhere, "{element:?} reads the same in two parts");
            }
        }
    }

    #[test]
    fn a_whole_part_pick_names_the_part_in_both_halves_of_the_id() {
        let pick = Pick::part(PartId(5));
        let id = encode_pick(&pick);
        assert_eq!(id & ELEMENT_MASK, PART_OFFSET + 6);
        assert_eq!(id >> PART_SHIFT, 5);
        assert_eq!(decode_pick(id), Some(pick));
    }

    #[test]
    fn the_first_parts_ids_are_the_within_part_ids() {
        // Vertex buffers carry the within-part ID; the shaders add the
        // part. For part zero the two agree, which is what lets a scene of
        // one part keep the IDs it always had.
        let face = TopologyElement::Face(FaceId(9));
        assert_eq!(
            encode_pick(&Pick::Solid {
                part: PartId(0),
                element: face.clone()
            }),
            encode_picking_id(&face)
        );
    }

    #[test]
    fn colour_encoding_roundtrip() {
        let id = 0x00_AB_CD_EF;
        let colour = id_to_colour(id);
        let decoded = colour_to_id(colour);
        assert_eq!(decoded, id);
    }
}
