// Render pipeline — orchestrates the main shaded pass, wireframe overlay,
// picking pass, and selection glow effect.

use bytemuck::{Pod, Zeroable};

use cadmark_core::sketch::SketchProfile;

use crate::camera::Camera;
use crate::mesh::{
    CORNER_TINT, EdgeVertex, GpuMesh, GpuSketch, GpuVertex, REGION_TINT, SketchVertex,
};
use crate::section::SectionPlane;

/// Opacity of the shaded surface while the model is see-through. Low enough
/// that an internal pocket reads through the near wall, high enough that the
/// near wall is still visibly there.
pub const TRANSPARENT_ALPHA: f32 = 0.35;

/// One application-owned viewport marker. The renderer receives topology IDs
/// and colours only; it does not know why an element is marked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportMarker {
    pub element_id: u32,
    pub colour: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MarkerGpu {
    element_id: u32,
    _padding: [u32; 3],
    colour: [f32; 4],
}

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
    /// How many entries of the highlight storage buffer the shaders read.
    pub highlight_count: u32,
    pub _pad3: u32,
    /// Number of live entries at the front of the marker storage buffer.
    pub marker_count: u32,
    /// How far the solid fades towards the background: 0 draws it
    /// normally, 1 leaves only a trace of it behind a sketch profile.
    pub ghost: f32,
    pub _pad5: [u32; 2],
    pub selected_colour: [f32; 4],
    pub hover_colour: [f32; 4],
    /// Section plane as `[nx, ny, nz, d]`; a fragment is discarded when
    /// `dot(n, world_pos) + d < 0`. All zeroes when no section is active,
    /// which keeps every fragment without a second flag.
    pub section_plane: [f32; 4],
    /// Opacity of the shaded surface: 1.0 solid, less when the user has
    /// switched the model to see-through.
    pub mesh_alpha: f32,
    pub _pad6: f32,
    pub _pad7: f32,
    pub _pad8: f32,
}

/// Initial length of the highlight storage buffer. It grows to whatever a
/// frame needs, so this bounds nothing — it only avoids reallocating for
/// the footprints small enough to be common.
const HIGHLIGHT_INITIAL_CAPACITY: u64 = 64;

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
    /// Picking IDs of the candidate-footprint highlight, read by the mesh
    /// and wireframe fragment shaders. Grown to fit each frame's set.
    pub highlight_buffer: wgpu::Buffer,
    marker_buffer: wgpu::Buffer,
    marker_capacity: usize,

    pub picking_pipeline: wgpu::RenderPipeline,
    pub picking_uniform_buffer: wgpu::Buffer,
    pub picking_bind_group: wgpu::BindGroup,
    pub edge_picking_pipeline: wgpu::RenderPipeline,

    /// Wireframe overlay; reads `mesh_uniform_buffer` through `mesh_bind_group`.
    pub wireframe_pipeline: wgpu::RenderPipeline,

    /// Sketch profile curves as lines, and its regions and corner markers
    /// as triangles. Both read `mesh_uniform_buffer` through
    /// `mesh_bind_group` and ignore depth, so the profile draws in front
    /// of any solid behind it.
    pub sketch_curve_pipeline: wgpu::RenderPipeline,
    pub sketch_fill_pipeline: wgpu::RenderPipeline,

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
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let mesh_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh_uniforms"),
            size: std::mem::size_of::<MeshUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let highlight_buffer = new_highlight_buffer(device, HIGHLIGHT_INITIAL_CAPACITY);
        let marker_buffer = new_marker_buffer(device, 1);
        let mesh_bind_group = mesh_bind_group(
            device,
            &mesh_bind_group_layout,
            &mesh_uniform_buffer,
            &highlight_buffer,
            &marker_buffer,
        );

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

        // -- Sketch profile pipelines --
        let sketch_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sketch_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sketch.wgsl").into()),
        });

        let sketch_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sketch_pipeline_layout"),
                bind_group_layouts: &[&mesh_bind_group_layout],
                push_constant_ranges: &[],
            });

        let sketch_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SketchVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, // position
                1 => Float32,   // tint
            ],
        };

        // Always passing the depth test, and writing none, is what puts the
        // profile in front: a solid the sketch will become no longer hides
        // the sketch that describes it.
        let sketch_depth = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: Default::default(),
            bias: Default::default(),
        };

        let sketch_curve_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sketch_curve_pipeline"),
                layout: Some(&sketch_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &sketch_shader,
                    entry_point: Some("vs_main"),
                    buffers: std::slice::from_ref(&sketch_vertex_layout),
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &sketch_shader,
                    entry_point: Some("fs_curve"),
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
                depth_stencil: Some(sketch_depth.clone()),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });

        let sketch_fill_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sketch_fill_pipeline"),
            layout: Some(&sketch_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &sketch_shader,
                entry_point: Some("vs_main"),
                buffers: &[sketch_vertex_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &sketch_shader,
                entry_point: Some("fs_fill"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    // Regions wash their area, so the ghosted solid behind
                    // them stays visible.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // A profile is drawn from both sides: the plane it sits on
                // may face away from the camera.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(sketch_depth),
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
            highlight_buffer,
            marker_buffer,
            marker_capacity: 1,
            picking_pipeline,
            picking_uniform_buffer,
            picking_bind_group,
            edge_picking_pipeline,
            wireframe_pipeline,
            sketch_curve_pipeline,
            sketch_fill_pipeline,
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
    /// Picking IDs of a secondary highlight — every element the ledger
    /// attributes to one candidate source line. Unbounded: the buffer the
    /// shaders read grows to hold whatever a footprint contains.
    pub highlight_ids: Vec<u32>,
    /// Application-provided topology markers, coloured to pair with UI cards.
    pub markers: Vec<ViewportMarker>,
    /// Whether the colour target stores sRGB-encoded values itself. When it
    /// does not, the shader gamma-encodes its output.
    pub target_is_srgb: bool,
    /// Whether the solid is faded back behind a sketch profile.
    pub ghost_solid: bool,
    /// The half-space the viewport keeps. Disabled by default.
    pub section: SectionPlane,
    /// Whether the shaded surface is drawn see-through.
    pub transparent: bool,
}

/// How far a ghosted solid fades towards the background. Enough that the
/// sketch reads as the live geometry, not so far that the solid it will
/// become is lost.
pub const GHOST_STRENGTH: f32 = 0.90;

impl Renderer {
    pub fn new() -> Self {
        Self {
            camera: Camera::default(),
            selection_style: SelectionStyle::default(),
            selected_id: 0,
            hover_id: 0,
            highlight_ids: Vec::new(),
            markers: Vec::new(),
            target_is_srgb: false,
            ghost_solid: false,
            section: SectionPlane::default(),
            transparent: false,
        }
    }

    /// Opacity the shaded pass draws at this frame.
    ///
    /// Ghosting wins over see-through. A ghosted solid is already faded
    /// towards the background so the sketch in front of it reads as the
    /// live geometry; blending that same near-background colour again at
    /// a third of its opacity would erase it, and the sketch would then
    /// have nothing to read as being in front of. The see-through
    /// setting is kept, not cleared, and takes effect again the moment
    /// there is no sketch to ghost behind.
    pub fn mesh_alpha(&self) -> f32 {
        if self.transparent && !self.ghost_solid {
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
            highlight_count: self.highlight_ids.len() as u32,
            _pad3: 0,
            marker_count: self.markers.len().try_into().unwrap_or(u32::MAX),
            ghost: if self.ghost_solid {
                GHOST_STRENGTH
            } else {
                0.0
            },
            _pad5: [0; 2],
            selected_colour: self.selection_style.selected_colour,
            hover_colour: self.selection_style.hover_colour,
            section_plane: self.section.equation(),
            mesh_alpha: self.mesh_alpha(),
            _pad6: 0.0,
            _pad7: 0.0,
            _pad8: 0.0,
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

/// A storage buffer able to hold `capacity` picking IDs.
fn new_highlight_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("highlight_ids"),
        size: capacity * std::mem::size_of::<u32>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn new_marker_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("viewport_markers"),
        size: (capacity * std::mem::size_of::<MarkerGpu>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn mesh_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    highlights: &wgpu::Buffer,
    markers: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mesh_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: highlights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: markers.as_entire_binding(),
            },
        ],
    })
}

/// How long a highlight buffer must be to hold `wanted` IDs: the current
/// length while it already fits, and otherwise doubled until it does.
///
/// Nothing here caps the answer. A candidate's footprint is however much
/// geometry the ledger attributes to its operation, and drawing only part
/// of it would show the user a smaller origin than the line actually has.
pub fn grown_highlight_capacity(current: u64, wanted: u64) -> u64 {
    let mut capacity = current.max(HIGHLIGHT_INITIAL_CAPACITY);
    while capacity < wanted {
        capacity *= 2;
    }
    capacity
}

impl RenderPipelines {
    /// Make room for `count` highlight IDs, rebuilding the bind group when
    /// the buffer had to be replaced. Returns whether it grew.
    pub fn reserve_highlights(&mut self, device: &wgpu::Device, count: usize) -> bool {
        let current = self.highlight_buffer.size() / std::mem::size_of::<u32>() as u64;
        let wanted = grown_highlight_capacity(current, count as u64);
        if wanted == current {
            return false;
        }
        self.highlight_buffer = new_highlight_buffer(device, wanted);
        self.mesh_bind_group = mesh_bind_group(
            device,
            &self.mesh_bind_group_layout,
            &self.mesh_uniform_buffer,
            &self.highlight_buffer,
            &self.marker_buffer,
        );
        true
    }

    /// Upload generic topology markers for the next viewport pass.
    pub fn set_markers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        markers: &[ViewportMarker],
    ) {
        let required = markers.len().max(1);
        if required > self.marker_capacity {
            self.marker_buffer = new_marker_buffer(device, required);
            self.marker_capacity = required;
            self.mesh_bind_group = mesh_bind_group(
                device,
                &self.mesh_bind_group_layout,
                &self.mesh_uniform_buffer,
                &self.highlight_buffer,
                &self.marker_buffer,
            );
        }
        let data: Vec<_> = markers
            .iter()
            .map(|marker| MarkerGpu {
                element_id: marker.element_id,
                _padding: [0; 3],
                colour: marker.colour,
            })
            .collect();
        if !data.is_empty() {
            queue.write_buffer(&self.marker_buffer, 0, bytemuck::cast_slice(&data));
        }
    }
}

/// A corner marker's half-width, as a fraction of the profile's extent, so
/// corners read as dots at any size of sketch.
const CORNER_MARKER_SCALE: f32 = 0.006;

/// The profile's curves as a line list: each polyline becomes its
/// segments, so a curve of any shape draws with one pipeline.
pub fn sketch_curve_vertices(profile: &SketchProfile) -> Vec<SketchVertex> {
    let mut vertices = Vec::new();
    for curve in &profile.curves {
        for window in curve.points.windows(2) {
            for &position in window {
                vertices.push(SketchVertex {
                    position,
                    tint: REGION_TINT,
                });
            }
        }
    }
    vertices
}

/// The profile's enclosed regions and corner markers as one triangle
/// list. Corners are quads lying in the sketch's own plane — the first
/// point geometry CADmark draws, and it lives here rather than in the
/// solid mesh because a solid has no points to show.
pub fn sketch_fill_vertices(profile: &SketchProfile) -> Vec<SketchVertex> {
    let mut vertices = Vec::new();
    for region in &profile.regions {
        for &index in &region.indices {
            let Some(&position) = region.vertices.get(index as usize) else {
                continue;
            };
            vertices.push(SketchVertex {
                position,
                tint: REGION_TINT,
            });
        }
    }

    let half_width = (profile.extent() * CORNER_MARKER_SCALE).max(f32::MIN_POSITIVE);
    let across = normalise(profile.plane.x_axis);
    let up = normalise(cross(profile.plane.normal, across));
    for corner in &profile.corners {
        let offset = |along: f32, sideways: f32| SketchVertex {
            position: std::array::from_fn(|axis| {
                corner.position[axis]
                    + across[axis] * along * half_width
                    + up[axis] * sideways * half_width
            }),
            tint: CORNER_TINT,
        };
        let quad = [
            offset(-1.0, -1.0),
            offset(1.0, -1.0),
            offset(1.0, 1.0),
            offset(-1.0, -1.0),
            offset(1.0, 1.0),
            offset(-1.0, 1.0),
        ];
        vertices.extend(quad);
    }
    vertices
}

/// Upload a sketch profile to GPU buffers.
pub fn upload_sketch(device: &wgpu::Device, profile: &SketchProfile) -> GpuSketch {
    use wgpu::util::DeviceExt;

    let curve_vertices = sketch_curve_vertices(profile);
    let fill_vertices = sketch_fill_vertices(profile);

    GpuSketch {
        curve_vertex_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sketch_curve_vertex_buffer"),
            contents: bytemuck::cast_slice(&curve_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        }),
        curve_vertex_count: curve_vertices.len() as u32,
        fill_vertex_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sketch_fill_vertex_buffer"),
            contents: bytemuck::cast_slice(&fill_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        }),
        fill_vertex_count: fill_vertices.len() as u32,
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalise(vector: [f32; 3]) -> [f32; 3] {
    let length = vector.iter().map(|c| c * c).sum::<f32>().sqrt();
    if length < 1e-6 {
        return [1.0, 0.0, 0.0];
    }
    vector.map(|component| component / length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::util::DeviceExt;

    /// Compile a shader and report its `Uniforms` struct as the field names
    /// and byte offsets the GPU will actually read, which is what the uniform
    /// buffer must match. Sizes alone do not say this: two same-width fields
    /// in the wrong order leave the total unchanged while every read after
    /// them lands on the wrong word.
    fn uniform_struct_layout(source: &str) -> Vec<(String, u32)> {
        let module = naga::front::wgsl::parse_str(source).expect("shader must compile");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        );
        validator.validate(&module).expect("shader must validate");
        let mut layouter = naga::proc::Layouter::default();
        layouter
            .update(module.to_ctx())
            .expect("shader types must lay out");
        let (handle, ty) = module
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some("Uniforms"))
            .expect("shader must declare Uniforms");
        let naga::TypeInner::Struct { members, .. } = &ty.inner else {
            panic!("Uniforms must be a struct");
        };
        let fields: Vec<(String, u32)> = members
            .iter()
            .map(|member| {
                (
                    member
                        .name
                        .clone()
                        .expect("every Uniforms field must be named"),
                    member.offset,
                )
            })
            .collect();
        assert_eq!(
            layouter[handle].size,
            std::mem::size_of::<MeshUniforms>() as u32,
            "the shader's Uniforms is a different size from MeshUniforms"
        );
        fields
    }

    #[test]
    fn every_shader_binding_mesh_uniforms_declares_the_same_layout() {
        // A field added, removed, renamed or reordered on one side and not
        // the other renders garbage silently, so the offset of every named
        // field is compared against the Rust struct rather than trusted.
        let expected: Vec<(String, u32)> = vec![
            ("view_proj", std::mem::offset_of!(MeshUniforms, view_proj)),
            ("eye_pos", std::mem::offset_of!(MeshUniforms, eye_pos)),
            (
                "encode_srgb",
                std::mem::offset_of!(MeshUniforms, encode_srgb),
            ),
            (
                "key_light_dir",
                std::mem::offset_of!(MeshUniforms, key_light_dir),
            ),
            ("_pad0", std::mem::offset_of!(MeshUniforms, _pad0)),
            (
                "fill_light_dir",
                std::mem::offset_of!(MeshUniforms, fill_light_dir),
            ),
            ("_pad1", std::mem::offset_of!(MeshUniforms, _pad1)),
            (
                "selected_id",
                std::mem::offset_of!(MeshUniforms, selected_id),
            ),
            ("hover_id", std::mem::offset_of!(MeshUniforms, hover_id)),
            (
                "highlight_count",
                std::mem::offset_of!(MeshUniforms, highlight_count),
            ),
            ("_pad3", std::mem::offset_of!(MeshUniforms, _pad3)),
            (
                "marker_count",
                std::mem::offset_of!(MeshUniforms, marker_count),
            ),
            ("ghost", std::mem::offset_of!(MeshUniforms, ghost)),
            ("_pad5", std::mem::offset_of!(MeshUniforms, _pad5)),
            (
                "selected_colour",
                std::mem::offset_of!(MeshUniforms, selected_colour),
            ),
            (
                "hover_colour",
                std::mem::offset_of!(MeshUniforms, hover_colour),
            ),
            (
                "section_plane",
                std::mem::offset_of!(MeshUniforms, section_plane),
            ),
            ("mesh_alpha", std::mem::offset_of!(MeshUniforms, mesh_alpha)),
            ("_pad6", std::mem::offset_of!(MeshUniforms, _pad6)),
            ("_pad7", std::mem::offset_of!(MeshUniforms, _pad7)),
            ("_pad8", std::mem::offset_of!(MeshUniforms, _pad8)),
        ]
        .into_iter()
        .map(|(name, offset)| (name.to_string(), offset as u32))
        .collect();

        assert_eq!(
            uniform_struct_layout(include_str!("shaders/mesh.wgsl")),
            expected
        );
        assert_eq!(
            uniform_struct_layout(include_str!("shaders/wireframe.wgsl")),
            expected
        );
    }

    #[test]
    fn every_element_of_a_footprint_is_handed_to_the_shaders() {
        // A fillet chain's footprint routinely runs to hundreds of edges;
        // showing part of one would understate what the line accounts for.
        let mut renderer = Renderer::new();
        renderer.highlight_ids = (1..=500).collect();
        assert_eq!(renderer.mesh_uniforms(1.0).highlight_count, 500);
    }

    #[test]
    fn the_highlight_buffer_grows_to_whatever_a_footprint_needs() {
        assert_eq!(grown_highlight_capacity(64, 12), 64);
        assert_eq!(grown_highlight_capacity(64, 64), 64);
        assert_eq!(grown_highlight_capacity(64, 65), 128);
        assert_eq!(grown_highlight_capacity(64, 500), 512);
        assert_eq!(grown_highlight_capacity(512, 9_000), 16_384);
    }

    fn triangle_profile() -> SketchProfile {
        use cadmark_core::sketch::{SketchCorner, SketchCurve, SketchRegion};

        SketchProfile {
            plane: Default::default(),
            curves: vec![SketchCurve {
                curve_id: 0,
                // Three points, so two segments, so four line vertices.
                points: vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 3.0, 0.0]],
            }],
            corners: vec![SketchCorner {
                corner_id: 0,
                position: [4.0, 0.0, 0.0],
            }],
            regions: vec![SketchRegion {
                region_id: 0,
                vertices: vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 3.0, 0.0]],
                indices: vec![0, 1, 2],
            }],
        }
    }

    #[test]
    fn sketch_curves_become_line_segments() {
        let vertices = sketch_curve_vertices(&triangle_profile());
        assert_eq!(vertices.len(), 4);
        assert_eq!(vertices[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(vertices[1].position, [4.0, 0.0, 0.0]);
        assert_eq!(vertices[2].position, [4.0, 0.0, 0.0]);
        assert_eq!(vertices[3].position, [4.0, 3.0, 0.0]);
    }

    #[test]
    fn sketch_corners_become_markers_lying_in_the_sketch_plane() {
        let profile = triangle_profile();
        let vertices = sketch_fill_vertices(&profile);
        // The region's three vertices, then the corner marker's two
        // triangles.
        assert_eq!(vertices.len(), 3 + 6);
        assert!(
            vertices[..3]
                .iter()
                .all(|vertex| vertex.tint == REGION_TINT)
        );

        let marker = &vertices[3..];
        assert!(marker.iter().all(|vertex| vertex.tint == CORNER_TINT));
        // The marker is a small square around its corner, flat on the
        // plane the sketch was drawn on.
        let half_width = profile.extent() * CORNER_MARKER_SCALE;
        for vertex in marker {
            assert!(
                (vertex.position[2]).abs() < 1e-6,
                "{vertex:?} left the plane"
            );
            assert!((vertex.position[0] - 4.0).abs() - half_width < 1e-6);
            assert!(vertex.position[1].abs() - half_width < 1e-6);
        }
        assert!(half_width > 0.0, "a marker with no size draws nothing");
    }

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

    /// Ghosting and see-through both fade the solid, and stacking them
    /// erases it: the ghost already mixes the surface most of the way to
    /// the background colour, and blending *that* at a third of its
    /// opacity leaves the near-background result indistinguishable from
    /// the background. The sketch drawn in front would then have nothing
    /// behind it to read as being in front of. Ghosting wins.
    #[test]
    fn a_ghosted_solid_stays_opaque_even_when_see_through_is_on() {
        let mut renderer = Renderer::new();
        renderer.transparent = true;
        renderer.ghost_solid = true;

        let uniforms = renderer.mesh_uniforms(1.0);
        assert_eq!(
            uniforms.mesh_alpha, 1.0,
            "a ghosted solid must not also be alpha-blended away"
        );
        assert!(
            uniforms.ghost > 0.0,
            "the ghost fade is what the user sees instead"
        );

        // The setting is kept, not cleared: dropping the sketch brings
        // the see-through view straight back.
        renderer.ghost_solid = false;
        assert!(renderer.mesh_uniforms(1.0).mesh_alpha < 1.0);
    }

    /// A device for the tests that must actually run a pass. The software
    /// adapter is asked for first, because a host that has one gives every
    /// seat the same instrument; where none is installed the host's own
    /// adapter is taken instead, which is what the marker-layout test beside
    /// this one has always done. An adapter is required either way — a test
    /// that quietly skips reads green while proving nothing, which is the
    /// failure this whole readback exists to stop.
    fn verification_device() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let options = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: true,
            compatible_surface: None,
        };
        let adapter = pollster::block_on(instance.request_adapter(&options))
            .or_else(|| {
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    force_fallback_adapter: false,
                    ..options
                }))
            })
            .expect("a wgpu adapter is required for renderer verification");
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
            .expect("the verification adapter must yield a device")
    }

    #[test]
    fn every_pass_builds_against_a_real_device() {
        // WGSL is compiled when a pipeline is created, not when the crate
        // is: a shader that does not parse, a uniform struct the shader
        // declares differently, or a binding whose visibility does not cover
        // the stage that reads it all surface here and nowhere earlier.
        let (device, _queue) = verification_device();

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
    #[test]
    fn marker_uniforms_name_only_live_entries() {
        let mut renderer = Renderer::new();
        renderer.markers = vec![
            ViewportMarker {
                element_id: 4,
                colour: [0.8, 0.2, 0.1, 0.7],
            },
            ViewportMarker {
                element_id: 9,
                colour: [0.1, 0.5, 0.9, 0.7],
            },
        ];

        let uniforms = renderer.mesh_uniforms(1.0);
        assert_eq!(uniforms.marker_count, 2);

        renderer.markers.pop();
        assert_eq!(renderer.mesh_uniforms(1.0).marker_count, 1);

        renderer.markers.clear();
        assert_eq!(renderer.mesh_uniforms(1.0).marker_count, 0);
    }

    #[test]
    fn pipeline_accepts_marker_layout_with_initially_cleared_markers() {
        let instance = wgpu::Instance::default();
        let options = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        };
        let adapter = pollster::block_on(instance.request_adapter(&options))
            .or_else(|| {
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    force_fallback_adapter: false,
                    ..options
                }))
            })
            .expect("a wgpu adapter is required for renderer verification");
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("renderer-pipeline-test"),
                ..Default::default()
            },
            None,
        ))
        .expect("software adapter device is available");
        let pipelines = RenderPipelines::new(&device, wgpu::TextureFormat::Bgra8Unorm, 4, 4);

        let mesh = GpuMesh {
            vertex_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("test-mesh-vertices"),
                contents: bytemuck::cast_slice(&[GpuVertex {
                    position: [0.0; 3],
                    normal: [0.0, 0.0, 1.0],
                    face_id: 1.0,
                    _padding: 0.0,
                }]),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            index_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("test-mesh-indices"),
                contents: bytemuck::cast_slice(&[0u32, 0, 0]),
                usage: wgpu::BufferUsages::INDEX,
            }),
            index_count: 3,
            edge_vertex_buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("test-edge-vertices"),
                contents: bytemuck::cast_slice(&[
                    EdgeVertex {
                        position: [0.0; 3],
                        edge_id: 1.0,
                    },
                    EdgeVertex {
                        position: [0.0; 3],
                        edge_id: 1.0,
                    },
                ]),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            edge_vertex_count: 2,
        };
        let renderer = Renderer::new();
        queue.write_buffer(
            &pipelines.mesh_uniform_buffer,
            0,
            bytemuck::bytes_of(&renderer.mesh_uniforms(1.0)),
        );
        // Both shaded passes are exercised: the opaque one and the
        // alpha-blended one the see-through view selects.
        for transparent in [false, true] {
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            crate::viewport::render_scene(
                &mut encoder,
                &pipelines,
                Some(&mesh),
                None,
                wgpu::Color::BLACK,
                transparent,
            );
            queue.submit(Some(encoder.finish()));
            device.poll(wgpu::Maintain::Wait);
            assert!(
                pollster::block_on(device.pop_error_scope()).is_none(),
                "marker bindings and every draw pipeline must validate (transparent: {transparent})"
            );
        }
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
        let (device, queue) = verification_device();

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
