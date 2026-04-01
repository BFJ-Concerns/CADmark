// Viewport — eframe/wgpu integration for rendering into the egui viewport.
//
// Handles the wgpu render pass within eframe's paint callback,
// issues the picking readback, and reports which element was hovered.

use crate::mesh::GpuMesh;
use crate::picking::{self, PickingPass};
use crate::pipeline::{MeshUniforms, RenderPipelines, SimpleUniforms};

/// State passed into the wgpu paint callback each frame.
pub struct ViewportRenderState {
    pub pipelines: Option<RenderPipelines>,
    pub picking: Option<PickingPass>,
    pub mesh: Option<GpuMesh>,
    pub mesh_uniforms: MeshUniforms,
    pub simple_uniforms: SimpleUniforms,
    /// Screen-space coordinates to read back for picking (if a click occurred).
    pub pick_request: Option<(u32, u32)>,
}


/// Encode a render pass for the main shaded mesh + wireframe overlay.
pub fn render_scene(
    encoder: &mut wgpu::CommandEncoder,
    surface_view: &wgpu::TextureView,
    state: &ViewportRenderState,
) {
    let Some(pipelines) = &state.pipelines else {
        return;
    };
    let Some(mesh) = &state.mesh else {
        // No mesh loaded — clear to dark background.
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.118,
                        g: 0.118,
                        b: 0.137,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        return;
    };

    // Main shaded pass.
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mesh_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.118,
                        g: 0.118,
                        b: 0.137,
                        a: 1.0,
                    }),
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

        pass.set_pipeline(&pipelines.mesh_pipeline);
        pass.set_bind_group(0, &pipelines.mesh_bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }

    // Wireframe overlay pass.
    if mesh.edge_vertex_count > 0 {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("wireframe_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface_view,
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

        pass.set_pipeline(&pipelines.wireframe_pipeline);
        pass.set_bind_group(0, &pipelines.wireframe_bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.edge_vertex_buffer.slice(..));
        pass.draw(0..mesh.edge_vertex_count, 0..1);
    }
}

/// Encode the picking pass into the offscreen colour-ID texture.
pub fn render_picking(
    encoder: &mut wgpu::CommandEncoder,
    state: &ViewportRenderState,
) {
    let Some(pipelines) = &state.pipelines else {
        return;
    };
    let Some(picking) = &state.picking else {
        return;
    };
    let Some(mesh) = &state.mesh else {
        return;
    };

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("picking_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &picking.texture_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Clear to 0 = background (no element).
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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

/// Issue a copy from the picking texture to the staging buffer for a single pixel.
pub fn request_pick_readback(
    encoder: &mut wgpu::CommandEncoder,
    picking: &PickingPass,
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
            buffer: &picking.staging_buffer,
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

/// Read back the picking result from the staging buffer.
/// Must be called after the GPU has finished the copy (map the buffer first).
pub fn decode_pick_result(data: &[u8]) -> Option<cadmark_core::geometry::TopologyElement> {
    if data.len() < 4 {
        return None;
    }

    let pixel = [data[0], data[1], data[2], data[3]];
    let id = picking::colour_to_id(pixel);
    picking::decode_picking_id(id)
}
