// CADmark — AI-directed CAD modelling with spatial comments.
//
// Binary entry point. Wires together the core, kernel, renderer,
// UI, and bridge crates into an eframe application.

use eframe::egui;

mod config;
pub mod git_ops;
pub mod orchestrator;
mod state;

fn surface_error_action(error: wgpu::SurfaceError) -> eframe::egui_wgpu::SurfaceErrorAction {
    match error {
        error @ (wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
            log::warn!("Reconfiguring rendering surface after {error}");
            eframe::egui_wgpu::SurfaceErrorAction::RecreateSurface
        }
        error => {
            log::warn!("Dropped frame with error: {error}");
            eframe::egui_wgpu::SurfaceErrorAction::SkipFrame
        }
    }
}

fn main() -> eframe::Result<()> {
    env_logger::init();
    log::info!("Starting CADmark");

    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    wgpu_options.on_surface_error = std::sync::Arc::new(surface_error_action);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("CADmark"),
        wgpu_options,
        // Depth testing happens in the offscreen viewport pass (state.rs),
        // not in egui's render pass — the blit pipeline has no depth.
        ..Default::default()
    };

    eframe::run_native(
        "CADmark",
        options,
        Box::new(|cc| Ok(Box::new(state::CadmarkApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn outdated_surface_is_reconfigured_instead_of_skipped() {
        assert!(matches!(
            super::surface_error_action(wgpu::SurfaceError::Outdated),
            eframe::egui_wgpu::SurfaceErrorAction::RecreateSurface
        ));
    }
}
