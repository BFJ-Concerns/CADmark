// The viewport's render passes and pick readback, in one place. The
// application's paint callback owns the GPU resources and the timing;
// this module owns what each pass draws, so a new pass (a vertex-marker
// pass, a section-plane pass) is added here and drawn from there.

use crate::mesh::{GpuMesh, GpuSketch};
use crate::picking::{self, PickingPass};
use crate::pipeline::RenderPipelines;

/// Encode the shaded mesh, wireframe overlay and sketch profile into the
/// offscreen colour target with depth. Runs every frame; with neither a
/// mesh nor a sketch it only clears, so clearing the model leaves no
/// stale render behind.
pub fn render_scene(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &RenderPipelines,
    mesh: Option<&GpuMesh>,
    sketch: Option<&GpuSketch>,
    clear_colour: wgpu::Color,
) {
    render_scene_into(
        encoder,
        pipelines,
        mesh,
        sketch,
        clear_colour,
        &pipelines.viewport_colour_view,
    );
}

/// The same scene drawn into a caller-chosen colour target, so a target
/// that is read back rather than blitted to the screen goes through this
/// one pass description. `colour_target` must match the pipelines'
/// surface format and the size their depth texture was built for.
pub fn render_scene_into(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &RenderPipelines,
    mesh: Option<&GpuMesh>,
    sketch: Option<&GpuSketch>,
    clear_colour: wgpu::Color,
    colour_target: &wgpu::TextureView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("viewport_main_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: colour_target,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(clear_colour),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &pipelines.depth_texture,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.0),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        ..Default::default()
    });

    if let Some(mesh) = mesh {
        pass.set_pipeline(&pipelines.mesh_pipeline);
        pass.set_bind_group(0, &pipelines.mesh_bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);

        if mesh.edge_vertex_count > 0 {
            pass.set_pipeline(&pipelines.wireframe_pipeline);
            pass.set_bind_group(0, &pipelines.mesh_bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.edge_vertex_buffer.slice(..));
            pass.draw(0..mesh.edge_vertex_count, 0..1);
        }
    }

    // The profile comes last, and its pipelines ignore depth, so the
    // sketch reads in front of whatever solid was drawn behind it.
    let Some(sketch) = sketch else {
        return;
    };
    if sketch.fill_vertex_count > 0 {
        pass.set_pipeline(&pipelines.sketch_fill_pipeline);
        pass.set_bind_group(0, &pipelines.mesh_bind_group, &[]);
        pass.set_vertex_buffer(0, sketch.fill_vertex_buffer.slice(..));
        pass.draw(0..sketch.fill_vertex_count, 0..1);
    }
    if sketch.curve_vertex_count > 0 {
        pass.set_pipeline(&pipelines.sketch_curve_pipeline);
        pass.set_bind_group(0, &pipelines.mesh_bind_group, &[]);
        pass.set_vertex_buffer(0, sketch.curve_vertex_buffer.slice(..));
        pass.draw(0..sketch.curve_vertex_count, 0..1);
    }
}

/// Render every pickable element into the colour-ID texture: faces
/// first, then edges drawn over them so a cursor on an edge picks the
/// edge.
pub fn render_picking(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &RenderPipelines,
    picking: &PickingPass,
    mesh: &GpuMesh,
) {
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("picking_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &picking.texture_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Every channel must clear to zero: the texture is
                    // Rgba8Uint, so an alpha of 1.0 would land as the byte
                    // 1 and decode as a vertex rather than the background.
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &pipelines.depth_texture,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        pass.set_pipeline(&pipelines.picking_pipeline);
        pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }

    if mesh.edge_vertex_count > 0 {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("edge_picking_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &picking.texture_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &pipelines.depth_texture,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        pass.set_pipeline(&pipelines.edge_picking_pipeline);
        pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.edge_vertex_buffer.slice(..));
        pass.draw(0..mesh.edge_vertex_count, 0..1);
    }
}

/// Copy one pixel of the picking texture into `staging`, which must be at
/// least 256 bytes and mappable for reading.
pub fn copy_pick_pixel(
    encoder: &mut wgpu::CommandEncoder,
    picking: &PickingPass,
    staging: &wgpu::Buffer,
    x: u32,
    y: u32,
) {
    // Clamp to texture bounds.
    let x = x.min(picking.width.saturating_sub(1));
    let y = y.min(picking.height.saturating_sub(1));

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &picking.texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                // Rgba8Uint = 4 bytes per pixel, row alignment to 256 bytes.
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
}

/// Decode a mapped pick pixel into the element under it, or `None` for
/// the background.
pub fn decode_pick_result(data: &[u8]) -> Option<cadmark_core::geometry::TopologyElement> {
    if data.len() < 4 {
        return None;
    }
    let pixel = [data[0], data[1], data[2], data[3]];
    picking::decode_picking_id(picking::colour_to_id(pixel))
}

#[cfg(test)]
mod tests {
    use super::decode_pick_result;
    use cadmark_core::geometry::{FaceId, TopologyElement};

    #[test]
    fn a_mapped_pixel_decodes_to_its_element_or_the_background() {
        assert_eq!(decode_pick_result(&[0, 0, 0, 0]), None);
        assert_eq!(decode_pick_result(&[1, 0]), None);
        assert_eq!(
            decode_pick_result(&[4, 0, 0, 0, 9, 9]),
            Some(TopologyElement::Face(FaceId(3)))
        );
    }
}
