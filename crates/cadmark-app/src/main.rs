// CADmark — AI-directed CAD modelling with spatial comments.
//
// Binary entry point. Wires together the core, kernel, renderer,
// UI, and bridge crates into an eframe application.

use eframe::egui;

mod state;

fn main() -> eframe::Result<()> {
    env_logger::init();
    log::info!("Starting CADmark");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("CADmark"),
        ..Default::default()
    };

    eframe::run_native(
        "CADmark",
        options,
        Box::new(|cc| Ok(Box::new(state::CadmarkApp::new(cc)))),
    )
}
