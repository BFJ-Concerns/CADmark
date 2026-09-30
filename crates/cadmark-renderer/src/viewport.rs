// The viewport's render passes and pick readback, in one place. The
// application's paint callback owns the GPU resources and the timing;
// this module owns what each pass draws, so a new pass (a vertex-marker
// pass, a section-plane pass) is added here and drawn from there.

use crate::mesh::{GpuMesh, GpuSketch};
use crate::picking::{self, Pick, PickingPass, SelectionFilter};
use crate::pipeline::RenderPipelines;

/// Encode the shaded mesh, wireframe overlay and sketch profile into the
/// offscreen colour target with depth. Runs every frame; with neither a
/// mesh nor a sketch it only clears, so clearing the model leaves no
/// stale render behind.
pub fn render_scene(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &RenderPipelines,
    meshes: &[GpuMesh],
    sketch: Option<&GpuSketch>,
    clear_colour: wgpu::Color,
    transparent: bool,
) {
    render_scene_into(
        encoder,
        pipelines,
        meshes,
        sketch,
        clear_colour,
        transparent,
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
    sketch: Option<&GpuSketch>,
    clear_colour: wgpu::Color,
    transparent: bool,
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

    for mesh in meshes.iter().filter(|mesh| mesh.visible) {
        pass.set_pipeline(if transparent {
            &pipelines.mesh_transparent_pipeline
        } else {
            &pipelines.mesh_pipeline
        });
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

        // Markers last within the part, so a vertex reads in front of the
        // edges meeting at it.
        if mesh.marker_vertex_count > 0 {
            pass.set_pipeline(&pipelines.vertex_marker_pipeline);
            pass.set_bind_group(0, &pipelines.mesh_bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.marker_vertex_buffer.slice(..));
            pass.draw(0..mesh.marker_vertex_count, 0..1);
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

fn render_topology_depth(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &RenderPipelines,
    meshes: &[GpuMesh],
    pipeline: &wgpu::RenderPipeline,
) {
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
    for mesh in meshes.iter().filter(|mesh| mesh.visible) {
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }
}

/// Render every pickable element into the colour-ID texture: faces
/// first, then edges drawn over them, then vertex markers over those, so
/// a cursor on an edge picks the edge and one on a vertex picks the
/// vertex. A sketch profile's regions, curves and corners come last and
/// ignore depth, as the visible profile does, so a click on the drawing
/// reaches the drawing rather than the ghosted solid behind it.
///
/// Every visible part is a pick target, its IDs qualified by the part in
/// the shaders, so a click on any part names that part's element.
///
/// `filter` decides which kinds are drawn at all. A kind the filter
/// refuses leaves the ID texture holding whatever is behind it, so a
/// click there falls through rather than reading as empty space. The
/// depth prepass is unfiltered: occluded geometry must still lose to a
/// part in front of it however the user has narrowed the selection.
pub fn render_picking(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &RenderPipelines,
    picking: &PickingPass,
    meshes: &[GpuMesh],
    sketch: Option<&GpuSketch>,
    filter: SelectionFilter,
) {
    // First establish scene depth with every part, so a face behind
    // another part's face loses to it whatever the filter says. This pass
    // has no colour attachment.
    if filter.faces {
        render_topology_depth(
            encoder,
            pipelines,
            meshes,
            &pipelines.topology_depth_pipeline,
        );
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

        for mesh in meshes.iter().filter(|mesh| filter.faces && mesh.visible) {
            pass.set_pipeline(&pipelines.picking_pipeline);
            pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }

    // Face IDs use exact occlusion. Markers need a surface-slope allowance
    // across their wider targets; establish that depth only after faces.
    if filter.edges || filter.vertices {
        render_topology_depth(encoder, pipelines, meshes, &pipelines.marker_depth_pipeline);
    }

    for mesh in meshes
        .iter()
        .filter(|mesh| filter.edges && mesh.visible && mesh.edge_vertex_count > 0)
    {
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

    for mesh in meshes
        .iter()
        .filter(|mesh| filter.vertices && mesh.visible && mesh.marker_vertex_count > 0)
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("marker_picking_pass"),
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

        pass.set_pipeline(&pipelines.marker_picking_pipeline);
        pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.marker_vertex_buffer.slice(..));
        pass.draw(0..mesh.marker_vertex_count, 0..1);
    }

    if let Some(sketch) = sketch {
        render_sketch_picking(encoder, pipelines, picking, sketch, filter);
    }
}

/// The sketch profile's pick targets over whatever the solid passes left:
/// regions, then curves over them, then corners over those, the same
/// precedence as faces, edges and vertices. The filter's three toggles
/// apply to the three kinds of sketch element in turn.
fn render_sketch_picking(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &RenderPipelines,
    picking: &PickingPass,
    sketch: &GpuSketch,
    filter: SelectionFilter,
) {
    let draws: [(&wgpu::RenderPipeline, &wgpu::Buffer, u32, bool); 3] = [
        (
            &pipelines.sketch_region_picking_pipeline,
            &sketch.pick_region_vertex_buffer,
            sketch.pick_region_vertex_count,
            filter.faces,
        ),
        (
            &pipelines.sketch_curve_picking_pipeline,
            &sketch.pick_curve_vertex_buffer,
            sketch.pick_curve_vertex_count,
            filter.edges,
        ),
        (
            &pipelines.sketch_corner_picking_pipeline,
            &sketch.pick_corner_vertex_buffer,
            sketch.pick_corner_vertex_count,
            filter.vertices,
        ),
    ];
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("sketch_picking_pass"),
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
    for (pipeline, buffer, count, enabled) in draws {
        if !enabled || count == 0 {
            continue;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.draw(0..count, 0..1);
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
    for mesh in meshes.iter().filter(|mesh| mesh.visible) {
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
///
/// The filter is applied here as well as at draw time: a readback already
/// in flight when the user narrows the selection carries IDs the filter
/// now refuses, and honouring the old frame would select a kind the user
/// has just turned off.
pub fn decode_pick_result(data: &[u8], filter: SelectionFilter) -> Option<Pick> {
    if data.len() < 4 {
        return None;
    }
    let pixel = [data[0], data[1], data[2], data[3]];
    let picked = picking::decode_pick(picking::colour_to_id(pixel))?;
    filter.allows_pick(&picked).then_some(picked)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use cadmark_core::geometry::{
        FaceId, PartId, SketchElement, SketchElementKind, TopologyElement,
    };
    use cadmark_core::mesh::{MeshVertex, TessellatedMesh};

    use super::*;
    use crate::pipeline::{MeshUniforms, SimpleUniforms, upload_mesh};

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

    /// A triangle filling one half of the clip square, so two of them
    /// occupy disjoint screen regions and each can be looked for on its own.
    fn triangle_in_half(x_min: f32, x_max: f32, depth: f32) -> TessellatedMesh {
        let mut mesh = triangle(depth);
        mesh.vertices[0].position = [x_min, -0.9, depth];
        mesh.vertices[1].position = [x_max, -0.9, depth];
        mesh.vertices[2].position = [(x_min + x_max) / 2.0, 0.9, depth];
        mesh
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
        assert_eq!(
            decode_pick_result(&[0, 0, 0, 0], SelectionFilter::default()),
            None
        );
        assert_eq!(
            decode_pick_result(&[1, 0], SelectionFilter::default()),
            None
        );
        assert_eq!(
            decode_pick_result(&[4, 0, 0, 0, 9, 9], SelectionFilter::default()),
            Some(Pick::Solid {
                part: PartId(0),
                element: TopologyElement::Face(FaceId(3))
            })
        );
    }

    #[test]
    fn topology_picking_names_the_part_under_the_cursor_and_skips_a_hidden_one() {
        // Two parts, each with a face numbered zero, one in front of the
        // other. A pick must name the near part's face zero, not the far
        // part's; and once the near part is hidden it neither answers nor
        // occludes, so the same pixel names the far part's face.
        let (device, queue) = crate::test_device::shared().handles();
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
                section_plane: [0.0; 4],
                marker_size: crate::markers::MarkerSizing::default().extent(64, 64),
            }),
        );
        let near = upload_mesh(&device, &triangle(0.2), PartId(0), &[]);
        let far = upload_mesh(&device, &triangle(0.8), PartId(1), &[]);
        let mut meshes = [near, far];
        let picking = PickingPass::new(&device, 64, 64);

        let pick_at_centre = |meshes: &[GpuMesh]| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("topology_pick_part_test"),
            });
            render_picking(
                &mut encoder,
                &pipelines,
                &picking,
                meshes,
                None,
                SelectionFilter::default(),
            );
            copy_pick_pixel(&mut encoder, &picking, &picking.staging_buffer, 32, 32);
            queue.submit([encoder.finish()]);
            decode_pick_result(
                &read_pick_pixel(&device, &picking.staging_buffer),
                SelectionFilter::default(),
            )
        };

        assert_eq!(
            pick_at_centre(&meshes),
            Some(Pick::Solid {
                part: PartId(0),
                element: TopologyElement::Face(FaceId(0))
            }),
            "the near part's face answers, named with its part"
        );

        meshes[0].visible = false;
        assert_eq!(
            pick_at_centre(&meshes),
            Some(Pick::Solid {
                part: PartId(1),
                element: TopologyElement::Face(FaceId(0))
            }),
            "a hidden part neither answers nor occludes"
        );
    }

    #[test]
    fn main_scene_depth_keeps_a_near_surface_in_front_of_a_later_far_surface() {
        let (device, queue) = crate::test_device::shared().handles();
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
                selected_id: crate::picking::encode_pick(&Pick::Solid {
                    part: PartId(1),
                    element: TopologyElement::Face(FaceId(5)),
                }),
                hover_id: 0,
                highlight_count: 0,
                _pad3: 0,
                marker_count: 0,
                ghost: 0.0,
                _pad4: 0,
                _pad5: 0,
                selected_colour: [1.0, 0.0, 0.0, 1.0],
                hover_colour: [0.0; 4],
                section_plane: [0.0; 4],
                mesh_alpha: 1.0,
                _pad6: 0.0,
                _pad7: 0.0,
                _pad8: 0.0,
                marker_size: crate::markers::MarkerSizing::default().extent(64, 64),
                part_colours: crate::pipeline::PART_PALETTE,
            }),
        );
        let near = upload_mesh(&device, &triangle_with_face(0.2, 0), PartId(0), &[]);
        let far = upload_mesh(&device, &triangle_with_face(0.8, 5), PartId(1), &[]);
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
            None,
            wgpu::Color::BLACK,
            false,
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

    #[test]
    fn every_part_of_a_multi_part_scene_reaches_the_frame() {
        let (device, queue) = crate::test_device::shared().handles();
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
                selected_id: 0,
                hover_id: 0,
                highlight_count: 0,
                _pad3: 0,
                marker_count: 0,
                ghost: 0.0,
                _pad4: 0,
                _pad5: 0,
                selected_colour: [0.0; 4],
                hover_colour: [0.0; 4],
                section_plane: [0.0; 4],
                mesh_alpha: 1.0,
                _pad6: 0.0,
                _pad7: 0.0,
                _pad8: 0.0,
                marker_size: crate::markers::MarkerSizing::default().extent(64, 64),
                part_colours: crate::pipeline::PART_PALETTE,
            }),
        );
        // Two parts side by side: the left half is part zero's alone, the
        // right half part one's, so a frame missing either is legible as a
        // background pixel where that part's own triangle should be.
        let left = upload_mesh(&device, &triangle_in_half(-0.9, -0.1, 0.5), PartId(0), &[]);
        let right = upload_mesh(&device, &triangle_in_half(0.1, 0.9, 0.5), PartId(1), &[]);
        let colour = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("multi_part_scene_colour"),
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
        let staging = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        let left_staging = staging("multi_part_scene_left");
        let right_staging = staging("multi_part_scene_right");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("multi_part_scene"),
        });
        render_scene_into(
            &mut encoder,
            &pipelines,
            &[left, right],
            None,
            wgpu::Color::BLACK,
            false,
            &colour.create_view(&wgpu::TextureViewDescriptor::default()),
        );
        let mut sample = |x: u32, target: &wgpu::Buffer| {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &colour,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y: 40, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: target,
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
        };
        sample(16, &left_staging);
        sample(48, &right_staging);
        queue.submit([encoder.finish()]);

        let lit = |pixel: [u8; 4]| pixel[0] as u32 + pixel[1] as u32 + pixel[2] as u32 > 30;
        let left_pixel = read_pick_pixel(&device, &left_staging);
        let right_pixel = read_pick_pixel(&device, &right_staging);
        assert!(
            lit(left_pixel),
            "first part did not reach the frame: {left_pixel:?}"
        );
        assert!(
            lit(right_pixel),
            "a later part did not reach the frame: {right_pixel:?}"
        );
    }

    #[test]
    fn a_sketch_pixel_decodes_to_the_sketch_element_it_encodes() {
        // 400_001 = the first sketch curve, above the part range.
        assert_eq!(
            decode_pick_result(&(400_001u32).to_le_bytes(), SelectionFilter::default()),
            Some(Pick::Sketch(SketchElement {
                kind: SketchElementKind::Curve,
                index: 0,
            })),
        );
    }
}
