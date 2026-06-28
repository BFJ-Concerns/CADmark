// Render pipeline — orchestrates the main shaded pass, wireframe overlay,
// picking pass, and selection glow effect.

use bytemuck::{Pod, Zeroable};

use crate::camera::Camera;
use crate::mesh::{EdgeVertex, GpuMesh, GpuVertex};
use crate::picking::PickingPass;

/// Configuration for the selection glow effect.
#[derive(Debug, Clone)]
pub struct SelectionStyle {
    /// Glow colour for selected elements [R, G, B, A].
    pub selected_colour: [f32; 4],
    /// Hover highlight colour.
    pub hover_colour: [f32; 4],
}

impl Default for SelectionStyle {
    fn default() -> Self {
        Self {
            selected_colour: [0.3, 0.6, 1.0, 0.8],
            hover_colour: [0.5, 0.7, 1.0, 0.4],
        }
    }
}

/// Uniforms for the main mesh shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct MeshUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub eye_pos: [f32; 3],
    pub _pad0: f32,
    pub selected_id: u32,
    pub hover_id: u32,
    pub _pad1: u32,
    pub _pad2: u32,
    pub selected_colour: [f32; 4],
    pub hover_colour: [f32; 4],
}

/// Uniforms for the picking and wireframe shaders (just view_proj).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct SimpleUniforms {
    pub view_proj: [[f32; 4]; 4],
}

/// GPU pipelines and resources for rendering.
pub struct RenderPipelines {
    pub mesh_pipeline: wgpu::RenderPipeline,
    pub mesh_bind_group_layout: wgpu::BindGroupLayout,
    pub mesh_uniform_buffer: wgpu::Buffer,
    pub mesh_bind_group: wgpu::BindGroup,

    pub picking_pipeline: wgpu::RenderPipeline,
    pub picking_uniform_buffer: wgpu::Buffer,
    pub picking_bind_group: wgpu::BindGroup,
    pub edge_picking_pipeline: wgpu::RenderPipeline,

    pub wireframe_pipeline: wgpu::RenderPipeline,
    pub wireframe_uniform_buffer: wgpu::Buffer,
    pub wireframe_bind_group: wgpu::BindGroup,

    /// Shared bind group layout for picking and wireframe passes
    /// (single uniform buffer at binding 0, vertex-stage visibility).
    pub simple_bind_group_layout: wgpu::BindGroupLayout,

    pub depth_texture: wgpu::TextureView,

    // ── Offscreen viewport + blit ──────────────────────────────────
    // Mesh and wireframe pipelines require depth stencil, but the egui
    // paint callback's render pass has no depth attachment. We render
    // to this offscreen texture in prepare() (with depth), then blit
    // the result onto the egui render pass in paint().
    /// Offscreen colour target for the main viewport pass.
    pub viewport_colour_view: wgpu::TextureView,
    /// Blit pipeline — fullscreen triangle sampling viewport_colour_view.
    pub blit_pipeline: wgpu::RenderPipeline,
    pub blit_bind_group_layout: wgpu::BindGroupLayout,
    pub blit_bind_group: wgpu::BindGroup,
    pub blit_sampler: wgpu::Sampler,
    /// Stored so resize() can recreate the offscreen texture.
    surface_format: wgpu::TextureFormat,
}

impl RenderPipelines {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        // -- Main mesh pipeline --
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mesh.wgsl").into()),
        });

        let mesh_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("mesh_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let mesh_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh_uniforms"),
            size: std::mem::size_of::<MeshUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mesh_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh_bind_group"),
            layout: &mesh_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: mesh_uniform_buffer.as_entire_binding(),
            }],
        });

        let mesh_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh_pipeline_layout"),
            bind_group_layouts: &[&mesh_bind_group_layout],
            push_constant_ranges: &[],
        });

        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh_pipeline"),
            layout: Some(&mesh_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &mesh_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, // position
                        1 => Float32x3, // normal
                        2 => Float32,   // face_id
                        3 => Float32,   // _padding
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &mesh_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None, // CAD models may have non-manifold faces.
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        // Shared layout for picking and wireframe — both need a single
        // uniform buffer at binding 0 with vertex-stage visibility.
        let simple_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("simple_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        // -- Picking pipeline (Rgba8Uint target) --
        let picking_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("picking_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/picking.wgsl").into()),
        });

        let picking_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("picking_uniforms"),
            size: std::mem::size_of::<SimpleUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let picking_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("picking_bind_group"),
            layout: &simple_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: picking_uniform_buffer.as_entire_binding(),
            }],
        });

        let picking_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("picking_pipeline_layout"),
                bind_group_layouts: &[&simple_bind_group_layout],
                push_constant_ranges: &[],
            });

        let picking_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("picking_pipeline"),
            layout: Some(&picking_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &picking_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x3,
                        2 => Float32,
                        3 => Float32,
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &picking_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Uint,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let edge_picking_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("edge_picking_pipeline"),
                layout: Some(&picking_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &picking_shader,
                    entry_point: Some("vs_edge"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<EdgeVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3,
                            1 => Float32,
                        ],
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &picking_shader,
                    entry_point: Some("fs_edge"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Uint,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });

        // -- Wireframe pipeline --
        let wireframe_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wireframe_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/wireframe.wgsl").into()),
        });

        let wireframe_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wireframe_uniforms"),
            size: std::mem::size_of::<SimpleUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let wireframe_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wireframe_bind_group"),
            layout: &simple_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wireframe_uniform_buffer.as_entire_binding(),
            }],
        });

        let wireframe_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("wireframe_pipeline_layout"),
                bind_group_layouts: &[&simple_bind_group_layout],
                push_constant_ranges: &[],
            });

        let wireframe_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wireframe_pipeline"),
            layout: Some(&wireframe_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &wireframe_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<EdgeVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, // position
                        1 => Float32,   // edge_id
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &wireframe_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                // Wireframe renders on top of mesh — use LessEqual.
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let depth_texture = create_depth_texture(device, width, height);

        // -- Offscreen viewport colour target --
        let viewport_colour_view =
            create_viewport_colour_texture(device, surface_format, width, height);

        // -- Blit pipeline (fullscreen triangle, no depth) --
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blit.wgsl").into()),
        });

        let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let blit_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("blit_bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit_bind_group"),
            layout: &blit_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&viewport_colour_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&blit_sampler),
                },
            ],
        });

        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit_pipeline_layout"),
            bind_group_layouts: &[&blit_bind_group_layout],
            push_constant_ranges: &[],
        });

        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit_pipeline"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        Self {
            mesh_pipeline,
            mesh_bind_group_layout,
            mesh_uniform_buffer,
            mesh_bind_group,
            picking_pipeline,
            picking_uniform_buffer,
            picking_bind_group,
            edge_picking_pipeline,
            wireframe_pipeline,
            wireframe_uniform_buffer,
            wireframe_bind_group,
            simple_bind_group_layout,
            depth_texture,
            viewport_colour_view,
            blit_pipeline,
            blit_bind_group_layout,
            blit_bind_group,
            blit_sampler,
            surface_format,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.depth_texture = create_depth_texture(device, width, height);
        self.viewport_colour_view =
            create_viewport_colour_texture(device, self.surface_format, width, height);

        // Recreate the blit bind group — it references the texture view.
        self.blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit_bind_group"),
            layout: &self.blit_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.viewport_colour_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.blit_sampler),
                },
            ],
        });
    }
}

fn create_viewport_colour_texture(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("viewport_colour_texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        // Render target (in prepare) + sampled (in paint blit).
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

fn create_depth_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth_texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The complete rendering state for the viewport.
pub struct Renderer {
    pub camera: Camera,
    pub picking: Option<PickingPass>,
    pub pipelines: Option<RenderPipelines>,
    pub selection_style: SelectionStyle,
    /// The currently loaded mesh, if any.
    pub mesh: Option<GpuMesh>,
    /// Picking ID of the currently selected element (for the glow shader).
    pub selected_id: u32,
    /// Picking ID of the element under the cursor (for hover highlight).
    pub hover_id: u32,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            camera: Camera::default(),
            picking: None,
            pipelines: None,
            selection_style: SelectionStyle::default(),
            mesh: None,
            selected_id: 0,
            hover_id: 0,
        }
    }

    /// Initialise GPU resources that depend on the wgpu device.
    pub fn init_gpu(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) {
        self.picking = Some(PickingPass::new(device, width, height));
        self.pipelines = Some(RenderPipelines::new(device, surface_format, width, height));
    }

    /// Handle viewport resize.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if let Some(picking) = &mut self.picking {
            picking.resize(device, width, height);
        }
        if let Some(pipelines) = &mut self.pipelines {
            pipelines.resize(device, width, height);
        }
    }

    /// Build the mesh uniforms for the current frame.
    pub fn mesh_uniforms(&self, aspect_ratio: f32) -> MeshUniforms {
        let view = self.camera.view_matrix();
        let proj = self.camera.projection_matrix(aspect_ratio);
        let view_proj = mat4_mul(proj, view);

        MeshUniforms {
            view_proj,
            eye_pos: self.camera.eye_position(),
            _pad0: 0.0,
            selected_id: self.selected_id,
            hover_id: self.hover_id,
            _pad1: 0,
            _pad2: 0,
            selected_colour: self.selection_style.selected_colour,
            hover_colour: self.selection_style.hover_colour,
        }
    }

    /// Build view-projection-only uniforms.
    pub fn simple_uniforms(&self, aspect_ratio: f32) -> SimpleUniforms {
        let view = self.camera.view_matrix();
        let proj = self.camera.projection_matrix(aspect_ratio);
        SimpleUniforms {
            view_proj: mat4_mul(proj, view),
        }
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

/// 4x4 matrix multiplication (column-major).
fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut result = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            result[col][row] = (0..4).map(|k| a[k][row] * b[col][k]).sum();
        }
    }
    result
}

/// Upload a tessellated mesh to GPU buffers.
pub fn upload_mesh(
    device: &wgpu::Device,
    mesh: &cadmark_kernel::tessellation::TessellatedMesh,
) -> GpuMesh {
    use wgpu::util::DeviceExt;

    // Build GPU vertices with face IDs.
    let mut gpu_vertices = Vec::with_capacity(mesh.vertices.len());
    // Build a per-vertex face ID lookup from the per-triangle face IDs.
    let mut vertex_face_ids = vec![0u32; mesh.vertices.len()];
    for (tri_idx, face_id) in mesh.face_ids.iter().enumerate() {
        let base = tri_idx * 3;
        if base + 2 < mesh.indices.len() {
            for &idx in &mesh.indices[base..base + 3] {
                vertex_face_ids[idx as usize] = *face_id + 1; // +1 because 0 = background.
            }
        }
    }

    for (i, v) in mesh.vertices.iter().enumerate() {
        gpu_vertices.push(GpuVertex {
            position: v.position,
            normal: v.normal,
            face_id: vertex_face_ids[i] as f32,
            _padding: 0.0,
        });
    }

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mesh_vertex_buffer"),
        contents: bytemuck::cast_slice(&gpu_vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });

    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mesh_index_buffer"),
        contents: bytemuck::cast_slice(&mesh.indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    // Build edge vertices as line segments.
    let mut edge_vertices = Vec::new();
    for (edge_idx, edge) in mesh.edges.iter().enumerate() {
        let edge_id =
            crate::picking::encode_picking_id(&cadmark_core::geometry::TopologyElement::Edge(
                cadmark_core::geometry::EdgeId(edge_idx as u32),
            )) as f32;
        for window in edge.points.windows(2) {
            edge_vertices.push(EdgeVertex {
                position: window[0],
                edge_id,
            });
            edge_vertices.push(EdgeVertex {
                position: window[1],
                edge_id,
            });
        }
    }

    let edge_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("edge_vertex_buffer"),
        contents: bytemuck::cast_slice(&edge_vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });

    GpuMesh {
        vertex_buffer,
        index_buffer,
        index_count: mesh.indices.len() as u32,
        edge_vertex_buffer,
        edge_vertex_count: edge_vertices.len() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_vertices_use_picking_edge_ids() {
        let mesh = cadmark_kernel::tessellation::TessellatedMesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            face_ids: Vec::new(),
            face_shape_hashes: Vec::new(),
            edges: vec![cadmark_kernel::tessellation::MeshEdge {
                points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                shape_hash: 1,
            }],
        };

        let encoded = crate::picking::encode_picking_id(
            &cadmark_core::geometry::TopologyElement::Edge(cadmark_core::geometry::EdgeId(0)),
        ) as f32;

        let mut edge_vertices = Vec::new();
        for (edge_idx, edge) in mesh.edges.iter().enumerate() {
            let edge_id =
                crate::picking::encode_picking_id(&cadmark_core::geometry::TopologyElement::Edge(
                    cadmark_core::geometry::EdgeId(edge_idx as u32),
                )) as f32;
            for window in edge.points.windows(2) {
                edge_vertices.push(EdgeVertex {
                    position: window[0],
                    edge_id,
                });
                edge_vertices.push(EdgeVertex {
                    position: window[1],
                    edge_id,
                });
            }
        }

        assert_eq!(edge_vertices.len(), 2);
        assert_eq!(edge_vertices[0].edge_id, encoded);
        assert_eq!(edge_vertices[1].edge_id, encoded);
    }
}
