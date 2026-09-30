// CADmark — AI-directed CAD modelling with spatial comments.
//
// Binary entry point: parses the project folder argument, configures the
// window, and hands off to the application.

// Tests build every boundary-crossing type on its shared base with
// struct-update syntax, even when they name every field, so a field added
// later is filled in one place (crates/cadmark-core/tests/
// boundary_type_construction.rs holds them to it); clippy's complaint that
// such an update is redundant today is the point.
#![cfg_attr(test, allow(clippy::needless_update))]

use eframe::egui;

mod app;
mod clipboard;
pub mod git_ops;
mod launch;
pub mod orchestrator;
pub mod parts;
mod project;
mod reference_images;
mod render_source;
mod script_parameters;
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

    // A folder named on the command line opens directly; with none, the
    // start view asks which project to open and nothing is loaded until
    // it is answered.
    let target = launch::target_from_args(std::env::args());

    let wgpu_options = eframe::egui_wgpu::WgpuConfiguration {
        on_surface_error: std::sync::Arc::new(surface_error_action),
        ..Default::default()
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("CADmark")
            // Matches `StartupWMClass` in the desktop entry, so a desktop
            // environment pairs the window with the launcher's icon.
            .with_app_id("cadmark"),
        wgpu_options,
        // Depth testing happens in the offscreen viewport pass, not in
        // egui's render pass — the blit pipeline has no depth.
        ..Default::default()
    };

    eframe::run_native(
        "CADmark",
        options,
        Box::new(move |cc| Ok(Box::new(app::CadmarkApp::new(cc, target)))),
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
