// The viewport's render passes and pick readback, in one place. The
// application's paint callback owns the GPU resources and the timing;
// this module owns what each pass draws, so a new pass (a vertex-marker
// pass, a section-plane pass) is added here and drawn from there.

use crate::mesh::GpuMesh;
use crate::picking::{self, PickingPass};
use crate::pipeline::RenderPipelines;

/// Encode the shaded mesh and wireframe overlay into the offscreen colour
/// target with depth. Runs every frame; with no mesh it only clears, so
/// clearing the model leaves no stale render behind.
pub fn render_scene(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &RenderPipelines,
    meshes: &[GpuMesh],
    clear_colour: wgpu::Color,
) {
    render_scene_into(
        encoder,
        pipelines,
        meshes,
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
    meshes: &[GpuMesh],
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

    for mesh in meshes {
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
}

/// Render every pickable element into the colour-ID texture: faces
/// first, then edges drawn over them so a cursor on an edge picks the
/// edge.
pub fn render_picking(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &RenderPipelines,
    picking: &PickingPass,
    all_meshes: &[GpuMesh],
    meshes: &[GpuMesh],
) {
    // First establish scene depth with every part. The selected part's local
    // IDs are rendered afterwards, but hidden geometry must still lose to a
    // different part in front of it. This pass has no colour attachment: it
    // must not manufacture IDs for non-active parts.
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("topology_picking_depth_prepass"),
            color_attachments: &[],
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
        for mesh in all_meshes {
            pass.set_pipeline(&pipelines.topology_depth_pipeline);
            pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }
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
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        for mesh in meshes {
            pass.set_pipeline(&pipelines.picking_pipeline);
            pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }

    for mesh in meshes.iter().filter(|mesh| mesh.edge_vertex_count > 0) {
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

/// Render only whole-part IDs. This is intentionally a separate pass from
/// topology picking so a part selection can never decode as a face or edge.
pub fn render_part_picking(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &RenderPipelines,
    picking: &PickingPass,
    meshes: &[GpuMesh],
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("part_picking_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &picking.texture_view,
            resolve_target: None,
            ops: wgpu::Operations {
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
    for mesh in meshes {
        pass.set_pipeline(&pipelines.part_picking_pipeline);
        pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
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
    use std::sync::mpsc;

    use cadmark_core::geometry::{FaceId, PartId, TopologyElement};
    use cadmark_core::mesh::{MeshVertex, TessellatedMesh};

    use super::*;
    use crate::pipeline::{MeshUniforms, SimpleUniforms, upload_mesh};

    fn gpu_device() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .expect("renderer unit test requires a software or hardware adapter");
        pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("renderer_viewport_test_device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
            },
            None,
        ))
        .expect("renderer unit test requires a device")
    }

    fn triangle(depth: f32) -> TessellatedMesh {
        TessellatedMesh {
            vertices: vec![
                MeshVertex {
                    position: [-0.8, -0.8, depth],
                    normal: [0.0, 0.0, 1.0],
                },
                MeshVertex {
                    position: [0.8, -0.8, depth],
                    normal: [0.0, 0.0, 1.0],
                },
                MeshVertex {
                    position: [0.0, 0.8, depth],
                    normal: [0.0, 0.0, 1.0],
                },
            ],
            indices: vec![0, 1, 2],
            face_ids: vec![0],
            edges: Vec::new(),
        }
    }

    fn triangle_with_face(depth: f32, face_id: u32) -> TessellatedMesh {
        let mut mesh = triangle(depth);
        mesh.face_ids[0] = face_id;
        mesh
    }

    fn read_pick_pixel(device: &wgpu::Device, staging: &wgpu::Buffer) -> [u8; 4] {
        let (tx, rx) = mpsc::sync_channel(1);
        staging
            .slice(..4)
            .map_async(wgpu::MapMode::Read, move |result| {
                tx.send(result).unwrap();
            });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let data = staging.slice(..4).get_mapped_range();
        let pixel = [data[0], data[1], data[2], data[3]];
        drop(data);
        staging.unmap();
        pixel
    }

    #[test]
    fn a_mapped_pixel_decodes_to_its_element_or_the_background() {
        assert_eq!(decode_pick_result(&[0, 0, 0, 0]), None);
        assert_eq!(decode_pick_result(&[1, 0]), None);
        assert_eq!(
            decode_pick_result(&[4, 0, 0, 0, 9, 9]),
            Some(TopologyElement::Face(FaceId(3)))
        );
    }

    #[test]
    fn topology_picking_reads_the_active_face_but_not_an_occluded_one() {
        let (device, queue) = gpu_device();
        let pipelines = RenderPipelines::new(&device, wgpu::TextureFormat::Rgba8Unorm, 64, 64);
        queue.write_buffer(
            &pipelines.picking_uniform_buffer,
            0,
            bytemuck::bytes_of(&SimpleUniforms {
                view_proj: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            }),
        );
        let near = upload_mesh(&device, &triangle(0.2), PartId(0));
        let far = upload_mesh(&device, &triangle(0.8), PartId(1));
        let meshes = [near, far];
        let picking = PickingPass::new(&device, 64, 64);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("topology_pick_occlusion_test"),
        });
        render_picking(&mut encoder, &pipelines, &picking, &meshes, &meshes[1..]);
        copy_pick_pixel(&mut encoder, &picking, &picking.staging_buffer, 32, 32);
        queue.submit([encoder.finish()]);
        assert_eq!(
            read_pick_pixel(&device, &picking.staging_buffer),
            [0, 0, 0, 0]
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("topology_pick_active_face_test"),
        });
        render_picking(
            &mut encoder,
            &pipelines,
            &picking,
            &meshes[..1],
            &meshes[..1],
        );
        copy_pick_pixel(&mut encoder, &picking, &picking.staging_buffer, 32, 32);
        queue.submit([encoder.finish()]);
        assert_eq!(
            decode_pick_result(&read_pick_pixel(&device, &picking.staging_buffer)),
            Some(TopologyElement::Face(FaceId(0)))
        );
    }

    #[test]
    fn main_scene_depth_keeps_a_near_surface_in_front_of_a_later_far_surface() {
        let (device, queue) = gpu_device();
        let pipelines = RenderPipelines::new(&device, wgpu::TextureFormat::Rgba8Unorm, 64, 64);
        queue.write_buffer(
            &pipelines.mesh_uniform_buffer,
            0,
            bytemuck::bytes_of(&MeshUniforms {
                view_proj: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
                eye_pos: [0.0, 0.0, 2.0],
                encode_srgb: 1,
                key_light_dir: [0.0, 0.0, 1.0],
                _pad0: 0.0,
                fill_light_dir: [0.0, 0.0, 1.0],
                _pad1: 0.0,
                selected_id: 6,
                hover_id: 0,
                highlight_count: 0,
                _pad3: 0,
                marker_count: 0,
                _pad4: 0,
                selected_part_id: crate::picking::encode_picking_id(&TopologyElement::Part(
                    PartId(1),
                )),
                hover_part_id: 0,
                selected_colour: [1.0, 0.0, 0.0, 1.0],
                hover_colour: [0.0; 4],
            }),
        );
        let near = upload_mesh(&device, &triangle_with_face(0.2, 0), PartId(0));
        let far = upload_mesh(&device, &triangle_with_face(0.8, 5), PartId(1));
        let colour = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("main_scene_depth_test_colour"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("main_scene_depth_test_staging"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("main_scene_depth_test"),
        });
        render_scene_into(
            &mut encoder,
            &pipelines,
            &[near, far],
            wgpu::Color::BLACK,
            &colour.create_view(&wgpu::TextureViewDescriptor::default()),
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &colour,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 32, y: 32, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
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
        queue.submit([encoder.finish()]);

        let pixel = read_pick_pixel(&device, &staging);
        assert!(
            pixel[1] > 20,
            "far selected surface overwrote near surface: {pixel:?}"
        );
    }
}
