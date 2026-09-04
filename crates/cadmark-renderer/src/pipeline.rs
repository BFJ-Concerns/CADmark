// Render pipeline — orchestrates the main shaded pass, wireframe overlay,
// picking pass, and selection glow effect.

use bytemuck::{Pod, Zeroable};

use crate::camera::Camera;
use crate::mesh::{EdgeVertex, GpuMesh, GpuVertex};
use crate::section::SectionPlane;

/// Opacity of the shaded surface while the model is see-through. Low enough
/// that an internal pocket reads through the near wall, high enough that the
/// near wall is still visibly there.
pub const TRANSPARENT_ALPHA: f32 = 0.35;

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
        // Linear colour; the shader display-encodes when the target needs it.
        Self {
            selected_colour: [0.12, 0.42, 1.0, 0.7],
            hover_colour: [0.3, 0.6, 1.0, 0.35],
        }
    }
}

/// Uniforms shared by the mesh and wireframe shaders.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct MeshUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub eye_pos: [f32; 3],
    /// Non-zero when the shader must gamma-encode its linear result because
    /// the colour target is not an sRGB format.
    pub encode_srgb: u32,
    pub key_light_dir: [f32; 3],
    pub _pad0: f32,
    pub fill_light_dir: [f32; 3],
    pub _pad1: f32,
    pub selected_id: u32,
    pub hover_id: u32,
    pub _pad2: u32,
    pub _pad3: u32,
    pub selected_colour: [f32; 4],
    pub hover_colour: [f32; 4],
    /// Section plane as `[nx, ny, nz, d]`; a fragment is discarded when
    /// `dot(n, world_pos) + d < 0`. All zeroes when no section is active,
    /// which keeps every fragment without a second flag.
    pub section_plane: [f32; 4],
    /// Opacity of the shaded surface: 1.0 solid, less when the user has
    /// switched the model to see-through.
    pub mesh_alpha: f32,
    pub _pad4: f32,
    pub _pad5: f32,
    pub _pad6: f32,
}

/// Uniforms for the picking shader (just view_proj).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct SimpleUniforms {
    pub view_proj: [[f32; 4]; 4],
    /// The same section plane the visible passes use, in the same form.
    /// Picking must clip identically: a face the section has cut away must
    /// not still answer a click.
    pub section_plane: [f32; 4],
}

/// GPU pipelines and resources for rendering.
pub struct RenderPipelines {
    pub mesh_pipeline: wgpu::RenderPipeline,
    /// The shaded pass again, alpha-blended with depth writes off, for when
    /// the user has made the model see-through.
    pub mesh_transparent_pipeline: wgpu::RenderPipeline,
    pub mesh_bind_group_layout: wgpu::BindGroupLayout,
    pub mesh_uniform_buffer: wgpu::Buffer,
    pub mesh_bind_group: wgpu::BindGroup,

    pub picking_pipeline: wgpu::RenderPipeline,
    pub picking_uniform_buffer: wgpu::Buffer,
    pub picking_bind_group: wgpu::BindGroup,
    pub edge_picking_pipeline: wgpu::RenderPipeline,

    /// Wireframe overlay; reads `mesh_uniform_buffer` through `mesh_bind_group`.
    pub wireframe_pipeline: wgpu::RenderPipeline,

    /// Bind group layout for the picking passes (single uniform buffer at
    /// binding 0, vertex-stage visibility).
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

        // The see-through variant of the same pass. Depth writes are off so
        // a near surface cannot hide the interior behind it; the depth test
        // stays on so the wireframe overlay still resolves against the
        // surface it was drawn over.
        let mesh_transparent_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mesh_transparent_pipeline"),
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
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // As the opaque pass: CAD models may have non-manifold
                    // faces, and culling would drop an open shell's far side
                    // altogether — the one thing this mode exists to show.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });

        // Picking layout — a single uniform buffer at binding 0 with
        // vertex-stage visibility.
        let simple_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("simple_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // The fragment stage reads the section plane to discard
                    // clipped geometry, so it binds this too.
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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

        let wireframe_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("wireframe_pipeline_layout"),
                bind_group_layouts: &[&mesh_bind_group_layout],
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
            mesh_transparent_pipeline,
            mesh_bind_group_layout,
            mesh_uniform_buffer,
            mesh_bind_group,
            picking_pipeline,
            picking_uniform_buffer,
            picking_bind_group,
            edge_picking_pipeline,
            wireframe_pipeline,
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

/// The viewport's per-frame state: the camera, the selection and hover
/// IDs the shaders highlight, and the colour-space flag. GPU resources
/// live with the paint callback that uses them.
pub struct Renderer {
    pub camera: Camera,
    pub selection_style: SelectionStyle,
    /// Picking ID of the currently selected element (for the glow shader).
    pub selected_id: u32,
    /// Picking ID of the element under the cursor (for hover highlight).
    pub hover_id: u32,
    /// Whether the colour target stores sRGB-encoded values itself. When it
    /// does not, the shader gamma-encodes its output.
    pub target_is_srgb: bool,
    /// The half-space the viewport keeps. Disabled by default.
    pub section: SectionPlane,
    /// Whether the shaded surface is drawn see-through.
    pub transparent: bool,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            camera: Camera::default(),
            selection_style: SelectionStyle::default(),
            selected_id: 0,
            hover_id: 0,
            target_is_srgb: false,
            section: SectionPlane::default(),
            transparent: false,
        }
    }

    /// Opacity the shaded pass draws at this frame.
    pub fn mesh_alpha(&self) -> f32 {
        if self.transparent {
            TRANSPARENT_ALPHA
        } else {
            1.0
        }
    }

    /// Build the mesh uniforms for the current frame.
    pub fn mesh_uniforms(&self, aspect_ratio: f32) -> MeshUniforms {
        let view = self.camera.view_matrix();
        let proj = self.camera.projection_matrix(aspect_ratio);
        let view_proj = mat4_mul(proj, view);

        let lights = self.camera.light_rig();

        MeshUniforms {
            view_proj,
            eye_pos: self.camera.eye_position(),
            encode_srgb: u32::from(!self.target_is_srgb),
            key_light_dir: lights.key,
            _pad0: 0.0,
            fill_light_dir: lights.fill,
            _pad1: 0.0,
            selected_id: self.selected_id,
            hover_id: self.hover_id,
            _pad2: 0,
            _pad3: 0,
            selected_colour: self.selection_style.selected_colour,
            hover_colour: self.selection_style.hover_colour,
            section_plane: self.section.equation(),
            mesh_alpha: self.mesh_alpha(),
            _pad4: 0.0,
            _pad5: 0.0,
            _pad6: 0.0,
        }
    }

    /// Build view-projection-only uniforms.
    pub fn simple_uniforms(&self, aspect_ratio: f32) -> SimpleUniforms {
        let view = self.camera.view_matrix();
        let proj = self.camera.projection_matrix(aspect_ratio);
        SimpleUniforms {
            view_proj: mat4_mul(proj, view),
            section_plane: self.section.equation(),
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
pub fn upload_mesh(device: &wgpu::Device, mesh: &cadmark_core::mesh::TessellatedMesh) -> GpuMesh {
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
    let edge_vertices = edge_vertices(mesh);

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

fn edge_vertices(mesh: &cadmark_core::mesh::TessellatedMesh) -> Vec<EdgeVertex> {
    let mut edge_vertices = Vec::new();
    for edge in &mesh.edges {
        let edge_id =
            crate::picking::encode_picking_id(&cadmark_core::geometry::TopologyElement::Edge(
                cadmark_core::geometry::EdgeId(edge.edge_id),
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
    edge_vertices
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fields of the `Uniforms` struct in a WGSL source, as `(name, type)`.
    fn wgsl_uniform_fields(source: &str) -> Vec<(String, String)> {
        let body = source
            .split_once("struct Uniforms {")
            .expect("no Uniforms struct")
            .1
            .split_once('}')
            .expect("unterminated Uniforms struct")
            .0;

        body.lines()
            .map(|line| line.split("//").next().unwrap_or("").trim())
            .filter(|line| !line.is_empty())
            .map(|line| {
                let line = line.trim_end_matches(',');
                let (name, ty) = line.split_once(':').expect("field without a type");
                (name.trim().to_owned(), ty.trim().to_owned())
            })
            .collect()
    }

    /// Size of a WGSL uniform-address-space struct with these fields, under
    /// WGSL's alignment rules. Compared against the Rust struct, this is what
    /// catches a field added on one side and not the other.
    fn wgsl_struct_size(fields: &[(String, String)]) -> usize {
        let mut offset = 0usize;
        let mut struct_align = 1usize;
        for (_, ty) in fields {
            let (align, size) = match ty.as_str() {
                "f32" | "u32" | "i32" => (4, 4),
                "vec2<f32>" | "vec2<u32>" => (8, 8),
                "vec3<f32>" | "vec3<u32>" => (16, 12),
                "vec4<f32>" | "vec4<u32>" => (16, 16),
                "mat4x4<f32>" => (16, 64),
                other => panic!("unhandled WGSL type in Uniforms: {other}"),
            };
            struct_align = struct_align.max(align);
            offset = offset.div_ceil(align) * align + size;
        }
        offset.div_ceil(struct_align) * struct_align
    }

    const MESH_SHADER: &str = include_str!("shaders/mesh.wgsl");
    const WIREFRAME_SHADER: &str = include_str!("shaders/wireframe.wgsl");
    const PICKING_SHADER: &str = include_str!("shaders/picking.wgsl");

    #[test]
    fn every_shader_binding_mesh_uniforms_mirrors_the_whole_struct() {
        // A field on the Rust struct with no counterpart in a shader that
        // binds it does not fail to compile: the frame renders garbage
        // silently, and every field past the mismatch is misread.
        for (name, source) in [("mesh", MESH_SHADER), ("wireframe", WIREFRAME_SHADER)] {
            let fields = wgsl_uniform_fields(source);
            assert_eq!(
                wgsl_struct_size(&fields),
                std::mem::size_of::<MeshUniforms>(),
                "{name}.wgsl's Uniforms is not the same size as MeshUniforms"
            );
        }
    }

    #[test]
    fn the_two_shaders_sharing_mesh_uniforms_declare_it_identically() {
        // The two bind one buffer through one bind group. Mirroring a change
        // into one and not the other is the failure this catches.
        assert_eq!(
            wgsl_uniform_fields(MESH_SHADER),
            wgsl_uniform_fields(WIREFRAME_SHADER),
            "mesh.wgsl and wireframe.wgsl disagree about Uniforms"
        );
    }

    #[test]
    fn the_picking_shader_mirrors_the_whole_of_simple_uniforms() {
        assert_eq!(
            wgsl_struct_size(&wgsl_uniform_fields(PICKING_SHADER)),
            std::mem::size_of::<SimpleUniforms>(),
            "picking.wgsl's Uniforms is not the same size as SimpleUniforms"
        );
    }

    #[test]
    fn the_uniforms_carry_the_section_plane_to_the_visible_and_picking_passes() {
        // Both must clip identically, or the user clicks a face the section
        // cut away and selects something they cannot see.
        let mut renderer = Renderer::new();
        renderer.section = crate::section::SectionPlane {
            enabled: true,
            axis: crate::section::Axis::Y,
            offset: 3.0,
            flipped: true,
        };

        let expected = renderer.section.equation();
        assert_ne!(expected, [0.0; 4]);
        assert_eq!(renderer.mesh_uniforms(1.0).section_plane, expected);
        assert_eq!(renderer.simple_uniforms(1.0).section_plane, expected);
    }

    #[test]
    fn a_see_through_model_reaches_the_shader_as_alpha_below_one() {
        let mut renderer = Renderer::new();
        assert_eq!(renderer.mesh_uniforms(1.0).mesh_alpha, 1.0);

        renderer.transparent = true;
        let alpha = renderer.mesh_uniforms(1.0).mesh_alpha;
        assert!(
            (0.0..1.0).contains(&alpha),
            "see-through alpha {alpha} would draw solid or invisible"
        );
    }

    /// A device on the **software** adapter, which is the instrument the
    /// consumer ladder names for renderer work at the unit rung. Forcing the
    /// fallback is the whole point: a pass that happens to work on this
    /// machine's GPU proves nothing about the rung, so a hardware adapter is
    /// never substituted silently. `None` means the software adapter is
    /// absent and the instrument is unavailable.
    fn software_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: true,
            compatible_surface: None,
        }))?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).ok()
    }

    #[test]
    fn every_pass_builds_on_the_software_adapter() {
        // WGSL is compiled when a pipeline is created, not when the crate
        // is: a shader that does not parse, a uniform struct the shader
        // declares differently, or a binding whose visibility does not cover
        // the stage that reads it all surface here and nowhere earlier.
        let Some((device, _queue)) = software_device() else {
            panic!("no software adapter: the ladder's unit-rung instrument is unavailable");
        };

        let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = errors.clone();
        device.on_uncaptured_error(Box::new(move |error| {
            sink.lock().expect("error sink").push(error.to_string());
        }));

        let pipelines = RenderPipelines::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb, 64, 64);
        let _ = pipelines.surface_format;
        let _ = device.poll(wgpu::Maintain::Wait);

        let errors = errors.lock().expect("error sink").clone();
        assert!(
            errors.is_empty(),
            "building the passes reported: {errors:#?}"
        );
    }

    #[test]
    fn edge_vertices_use_picking_edge_ids() {
        let mesh = cadmark_core::mesh::TessellatedMesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            face_ids: Vec::new(),
            edges: vec![cadmark_core::mesh::MeshEdge {
                points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                edge_id: 7,
            }],
        };

        let encoded = crate::picking::encode_picking_id(
            &cadmark_core::geometry::TopologyElement::Edge(cadmark_core::geometry::EdgeId(7)),
        ) as f32;

        let edge_vertices = edge_vertices(&mesh);

        assert_eq!(edge_vertices.len(), 2);
        assert_eq!(edge_vertices[0].edge_id, encoded);
        assert_eq!(edge_vertices[1].edge_id, encoded);
    }

    /// One quad, one plane, and the picking texture read back on both sides
    /// of the cut.
    ///
    /// Building the pipeline only proves the shader compiles. What the user
    /// meets is the readback: click a face the section has cut away and the
    /// pick must come back empty, not with the face they cannot see. That is
    /// what this renders and reads.
    #[test]
    fn the_section_plane_makes_a_clipped_face_unpickable_in_the_readback() {
        let Some((device, queue)) = software_device() else {
            panic!("no software adapter: the ladder's unit-rung instrument is unavailable");
        };

        const SIZE: u32 = 64;
        let pipelines =
            RenderPipelines::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb, SIZE, SIZE);
        let picking = crate::picking::PickingPass::new(&device, SIZE, SIZE);

        // A quad across the whole viewport at z = 0, drawn under an identity
        // view-projection so a vertex position is also its NDC position and a
        // pixel maps to a world x by hand.
        let face = cadmark_core::geometry::TopologyElement::Face(cadmark_core::geometry::FaceId(3));
        let face_id = crate::picking::encode_picking_id(&face) as f32;
        let corner = |x: f32, y: f32| crate::mesh::GpuVertex {
            position: [x, y, 0.0],
            normal: [0.0, 0.0, 1.0],
            face_id,
            _padding: 0.0,
        };
        let vertices = [
            corner(-0.9, -0.9),
            corner(0.9, -0.9),
            corner(0.9, 0.9),
            corner(-0.9, 0.9),
        ];
        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];

        let mesh = crate::mesh::GpuMesh {
            vertex_buffer: buffer_of(
                &device,
                &queue,
                bytemuck::cast_slice(&vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            index_buffer: buffer_of(
                &device,
                &queue,
                bytemuck::cast_slice(&indices),
                wgpu::BufferUsages::INDEX,
            ),
            index_count: indices.len() as u32,
            edge_vertex_buffer: buffer_of(&device, &queue, &[0u8; 16], wgpu::BufferUsages::VERTEX),
            edge_vertex_count: 0,
        };

        // Cut at x = 0, keeping the low side.
        let section = crate::section::SectionPlane {
            enabled: true,
            axis: crate::section::Axis::X,
            offset: 0.0,
            flipped: false,
        };
        let uniforms = SimpleUniforms {
            view_proj: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            section_plane: section.equation(),
        };
        queue.write_buffer(
            &pipelines.picking_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        // A pixel's centre in NDC, so the sides are named by the same
        // arithmetic the shader clips by rather than by eye.
        let ndc_x = |px: u32| (px as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
        let (kept_px, clipped_px) = (SIZE / 4, SIZE * 3 / 4);
        assert!(
            section.keeps([ndc_x(kept_px), 0.0, 0.0])
                && !section.keeps([ndc_x(clipped_px), 0.0, 0.0]),
            "the sampled pixels do not straddle the plane"
        );

        let kept = read_pick(
            &device,
            &queue,
            &pipelines,
            &picking,
            &mesh,
            kept_px,
            SIZE / 2,
        );
        let clipped = read_pick(
            &device,
            &queue,
            &pipelines,
            &picking,
            &mesh,
            clipped_px,
            SIZE / 2,
        );

        assert_eq!(
            kept,
            Some(face),
            "the half the section keeps stopped answering a click"
        );
        assert_eq!(
            clipped, None,
            "a face the section cut away is still pickable: the user selects what they cannot see"
        );
    }

    fn buffer_of(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: data.len() as u64,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, data);
        buffer
    }

    /// Render the picking pass and read one pixel back, the way the
    /// application does when the user clicks.
    fn read_pick(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipelines: &RenderPipelines,
        picking: &crate::picking::PickingPass,
        mesh: &crate::mesh::GpuMesh,
        x: u32,
        y: u32,
    ) -> Option<cadmark_core::geometry::TopologyElement> {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        crate::viewport::render_picking(&mut encoder, pipelines, picking, mesh);
        crate::viewport::copy_pick_pixel(&mut encoder, picking, &picking.staging_buffer, x, y);
        queue.submit([encoder.finish()]);

        let slice = picking.staging_buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::Maintain::Wait);
        let element = crate::viewport::decode_pick_result(&slice.get_mapped_range());
        picking.staging_buffer.unmap();
        element
    }
}
