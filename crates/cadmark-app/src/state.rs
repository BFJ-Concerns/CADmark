// Application state — the top-level struct that owns all subsystems
// and implements the eframe::App trait.

use cadmark_core::geometry::SelectionState;
use cadmark_core::message::Conversation;
use cadmark_core::version::VersionHistory;
use cadmark_renderer::pipeline::Renderer;
use cadmark_ui::chat::ChatPane;
use cadmark_ui::overlay::OverlayState;

/// Top-level application state.
pub struct CadmarkApp {
    /// Conversation history displayed in the chat pane.
    pub conversation: Conversation,
    /// Chat pane UI state.
    pub chat: ChatPane,
    /// Spatial comment overlay state.
    pub overlay: OverlayState,
    /// Version history for undo/redo.
    pub history: VersionHistory,
    /// 3D renderer state.
    pub renderer: Renderer,
    /// Current selection in the viewport.
    pub selection: SelectionState,
    /// Path to the project directory.
    pub project_dir: Option<std::path::PathBuf>,
    /// Whether the AI is currently processing a request.
    pub ai_busy: bool,
}

impl CadmarkApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        // On first launch, add an initial AI message.
        let mut conversation = Conversation::new();
        conversation.push(cadmark_core::message::Message::ai_response(
            "Welcome to CADmark. Describe what you'd like to build, \
             or open a project directory to continue working.",
        ));

        Self {
            conversation,
            chat: ChatPane::new(),
            overlay: OverlayState::default(),
            history: VersionHistory::new(),
            renderer: Renderer::new(),
            selection: SelectionState::None,
            project_dir: None,
            ai_busy: false,
        }
    }
}

impl eframe::App for CadmarkApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Top toolbar with undo/redo.
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            let action = cadmark_ui::toolbar::show_toolbar(ui, &self.history);
            match action {
                cadmark_ui::toolbar::ToolbarAction::Undo => {
                    if let Some(_version) = self.history.undo() {
                        // TODO: Restore model state from this version's commit.
                        log::info!("Undo");
                    }
                }
                cadmark_ui::toolbar::ToolbarAction::Redo => {
                    if let Some(_version) = self.history.redo() {
                        log::info!("Redo");
                    }
                }
                cadmark_ui::toolbar::ToolbarAction::JumpToVersion(idx) => {
                    log::info!("Jump to version {idx}");
                }
                cadmark_ui::toolbar::ToolbarAction::None => {}
            }
        });

        // Right panel: chat pane.
        egui::SidePanel::right("chat_panel")
            .default_width(350.0)
            .show(ctx, |ui| {
                self.chat.is_loading = self.ai_busy;
                if let Some(message) = self.chat.show(ui, &self.conversation) {
                    // User submitted a chat message.
                    self.conversation
                        .push(cadmark_core::message::Message::user_chat(&message));
                    log::info!("User message: {message}");
                    // TODO: Send to AI backend.
                }
            });

        // Central panel: 3D viewport.
        egui::CentralPanel::default().show(ctx, |ui| {
            // Viewport area — will host the wgpu render surface.
            let available = ui.available_size();
            let (rect, _response) =
                ui.allocate_exact_size(available, egui::Sense::click_and_drag());

            // Placeholder: draw a dark background where the 3D viewport will be.
            ui.painter().rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgb(30, 30, 35),
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "3D Viewport",
                egui::FontId::proportional(18.0),
                egui::Color32::from_rgb(80, 80, 90),
            );

            // Show the spatial comment overlay if active.
            let overlay_action = self.overlay.show(ui);
            match overlay_action {
                cadmark_ui::overlay::OverlayAction::Submit(text) => {
                    log::info!("Spatial comment submitted: {text}");
                    self.overlay.close();
                    // TODO: Create spatial comment message with geometry context,
                    // send to AI backend.
                }
                cadmark_ui::overlay::OverlayAction::Cancel => {
                    self.overlay.close();
                    self.selection = SelectionState::None;
                }
                cadmark_ui::overlay::OverlayAction::None => {}
            }
        });
    }
}
