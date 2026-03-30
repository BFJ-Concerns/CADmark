// Render pipeline — orchestrates the main shaded pass, wireframe overlay,
// picking pass, and selection glow effect.

use crate::camera::Camera;
use crate::mesh::GpuMesh;
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

/// The complete rendering state for the viewport.
pub struct Renderer {
    pub camera: Camera,
    pub picking: Option<PickingPass>,
    pub selection_style: SelectionStyle,
    /// The currently loaded mesh, if any.
    pub mesh: Option<GpuMesh>,
    /// Face ID of the currently selected element (for the glow shader).
    pub selected_id: u32,
    /// Face ID of the element under the cursor (for hover highlight).
    pub hover_id: u32,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            camera: Camera::default(),
            picking: None,
            selection_style: SelectionStyle::default(),
            mesh: None,
            selected_id: 0,
            hover_id: 0,
        }
    }

    /// Initialise GPU resources that depend on the wgpu device.
    pub fn init_gpu(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.picking = Some(PickingPass::new(device, width, height));
    }

    /// Handle viewport resize.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if let Some(picking) = &mut self.picking {
            picking.resize(device, width, height);
        }
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}
