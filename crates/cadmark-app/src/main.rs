// CADmark — AI-directed CAD modelling with spatial comments.
//
// Binary entry point: parses the project folder argument, configures the
// window, and hands off to the application.

use eframe::egui;

mod app;
pub mod git_ops;
pub mod orchestrator;
mod project;
mod render_source;
pub mod turn;
mod user_settings;
mod validity;
mod viewport;

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

    // The project folder: the first argument, else the current directory.
    let project_dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("failed to read current directory"));

    let wgpu_options = eframe::egui_wgpu::WgpuConfiguration {
        on_surface_error: std::sync::Arc::new(surface_error_action),
        ..Default::default()
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("CADmark"),
        wgpu_options,
        // Depth testing happens in the offscreen viewport pass, not in
        // egui's render pass — the blit pipeline has no depth.
        ..Default::default()
    };

    eframe::run_native(
        "CADmark",
        options,
        Box::new(move |cc| Ok(Box::new(app::CadmarkApp::new(cc, project_dir)))),
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
