// GPU colour-ID picking — offscreen render pass that encodes each
// topological element as a unique colour.
//
// Faces, edges, and vertices occupy distinct ID ranges so the type
// and index can be recovered from a single pixel readback.

use cadmark_core::geometry::{EdgeId, FaceId, TopologyElement, VertexId};

/// ID ranges for each element type in the picking pass.
/// Face IDs:  1..EDGE_OFFSET-1
/// Edge IDs:  EDGE_OFFSET..VERTEX_OFFSET-1
/// Vertex IDs: VERTEX_OFFSET..
///
/// ID 0 = background (no element).
const EDGE_OFFSET: u32 = 100_000;
const VERTEX_OFFSET: u32 = 200_000;

/// Encode a topology element as a picking ID for the colour buffer.
pub fn encode_picking_id(element: &TopologyElement) -> u32 {
    match element {
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
    } else {
        Some(TopologyElement::Vertex(VertexId(id - VERTEX_OFFSET - 1)))
    }
}

/// Encode a picking ID as RGBA bytes for the colour attachment.
/// Uses R and G channels for the 32-bit ID (little-endian).
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
    fn background_decodes_to_none() {
        assert_eq!(decode_picking_id(0), None);
    }

    #[test]
    fn colour_encoding_roundtrip() {
        let id = 0x00_AB_CD_EF;
        let colour = id_to_colour(id);
        let decoded = colour_to_id(colour);
        assert_eq!(decoded, id);
    }
}
