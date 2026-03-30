// Application state — the top-level struct that owns all subsystems
// and implements the eframe::App trait.

use std::sync::mpsc;

use cadmark_core::geometry::SelectionState;
use cadmark_core::message::Conversation;
use cadmark_core::version::VersionHistory;
use cadmark_renderer::pipeline::Renderer;
use cadmark_ui::chat::ChatPane;
use cadmark_ui::overlay::OverlayState;

use crate::orchestrator::{OrchestratorCommand, OrchestratorResult};

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
    /// Channel to send commands to the orchestrator.
    pub cmd_tx: Option<mpsc::Sender<OrchestratorCommand>>,
    /// Channel to receive results from the orchestrator.
    pub result_rx: Option<mpsc::Receiver<OrchestratorResult>>,
}

impl CadmarkApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        // On first launch, add an initial AI message.
        let mut conversation = Conversation::new();
        conversation.push(cadmark_core::message::Message::ai_response(
            "Welcome to CADmark. Describe what you'd like to build, \
             or open a project directory to continue working.",
        ));

        // If a project dir was passed as a CLI argument, set it up.
        let project_dir = std::env::args().nth(1).map(std::path::PathBuf::from);

        let (cmd_tx, result_rx) = if let Some(ref dir) = project_dir {
            let (tx, rx) = crate::orchestrator::spawn_orchestrator(
                dir.clone(),
                "part.py".to_string(),
            );
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        Self {
            conversation,
            chat: ChatPane::new(),
            overlay: OverlayState::default(),
            history: VersionHistory::new(),
            renderer: Renderer::new(),
            selection: SelectionState::None,
            project_dir,
            ai_busy: false,
            cmd_tx,
            result_rx,
        }
    }

    /// Send a chat message to the AI backend.
    fn send_chat_message(&mut self, message: String) {
        self.conversation
            .push(cadmark_core::message::Message::user_chat(&message));

        if let Some(tx) = &self.cmd_tx {
            if tx.send(OrchestratorCommand::ChatMessage(message)).is_ok() {
                self.ai_busy = true;
            } else {
                self.conversation.push(cadmark_core::message::Message::ai_response(
                    "Error: AI backend is not available.",
                ));
            }
        } else {
            self.conversation.push(cadmark_core::message::Message::ai_response(
                "No project directory set. Pass a directory path as a command-line argument.",
            ));
        }
    }

    /// Send a spatial comment to the AI backend.
    fn send_spatial_comment(
        &mut self,
        text: String,
        context: cadmark_core::geometry::GeometryContext,
    ) {
        self.conversation.push(
            cadmark_core::message::Message::spatial_comment(&text, context.clone()),
        );

        if let Some(tx) = &self.cmd_tx {
            if tx
                .send(OrchestratorCommand::SpatialComment { text, context })
                .is_ok()
            {
                self.ai_busy = true;
            }
        }
    }

    /// Poll for orchestrator results (non-blocking).
    fn poll_results(&mut self) {
        if let Some(rx) = &self.result_rx {
            while let Ok(result) = rx.try_recv() {
                self.ai_busy = false;
                match result {
                    OrchestratorResult::Success { response, script_path } => {
                        // Mark all pending spatial comments as applied.
                        self.conversation.mark_all_spatial_applied();

                        // Add the AI response to chat.
                        self.conversation.push(
                            cadmark_core::message::Message::ai_response(&response.message),
                        );

                        // Create a microversion commit.
                        if let Some(ref dir) = self.project_dir {
                            match crate::git_ops::create_microversion(
                                dir,
                                &response.summary,
                                &response.message,
                                script_path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("part.py"),
                            ) {
                                Ok(version) => {
                                    self.history.push(version);
                                }
                                Err(e) => {
                                    log::warn!("Failed to create microversion: {e}");
                                }
                            }
                        }

                        // TODO: Re-execute script and update mesh in renderer.
                        log::info!("AI response received, script updated");
                    }
                    OrchestratorResult::ExecutionFailed { ai_message, error } => {
                        self.conversation.push(
                            cadmark_core::message::Message::ai_response(&format!(
                                "{ai_message}\n\n**Execution failed:**\n```\n{error}\n```"
                            )),
                        );
                    }
                    OrchestratorResult::BackendError(error) => {
                        self.conversation.push(
                            cadmark_core::message::Message::ai_response(&format!(
                                "**AI error:** {error}"
                            )),
                        );
                    }
                }
            }
        }
    }
}

impl eframe::App for CadmarkApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll for async results from the orchestrator.
        self.poll_results();

        // Request continuous repaints while AI is busy (to poll results).
        if self.ai_busy {
            ctx.request_repaint();
        }

        // Top toolbar with undo/redo.
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            let action = cadmark_ui::toolbar::show_toolbar(ui, &self.history);
            match action {
                cadmark_ui::toolbar::ToolbarAction::Undo => {
                    if let Some(version) = self.history.undo() {
                        if let Some(ref dir) = self.project_dir {
                            if let Err(e) =
                                crate::git_ops::checkout_commit(dir, &version.commit_hash)
                            {
                                log::error!("Undo failed: {e}");
                            }
                        }
                        log::info!("Undo to {}", version.commit_hash);
                    }
                }
                cadmark_ui::toolbar::ToolbarAction::Redo => {
                    if let Some(version) = self.history.redo() {
                        if let Some(ref dir) = self.project_dir {
                            if let Err(e) =
                                crate::git_ops::checkout_commit(dir, &version.commit_hash)
                            {
                                log::error!("Redo failed: {e}");
                            }
                        }
                        log::info!("Redo to {}", version.commit_hash);
                    }
                }
                cadmark_ui::toolbar::ToolbarAction::JumpToVersion(_idx) => {
                    // TODO: Jump to specific version.
                    log::info!("Jump to version");
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
                    self.send_chat_message(message);
                }
            });

        // Central panel: 3D viewport.
        egui::CentralPanel::default().show(ctx, |ui| {
            let available = ui.available_size();
            let (rect, response) =
                ui.allocate_exact_size(available, egui::Sense::click_and_drag());

            // Handle viewport input.
            if response.dragged_by(egui::PointerButton::Secondary) {
                let delta = response.drag_delta();
                self.renderer.camera.orbit(delta.x, delta.y);
            }

            if response.dragged_by(egui::PointerButton::Middle) {
                let delta = response.drag_delta();
                self.renderer.camera.pan(delta.x, delta.y);
            }

            // Scroll to zoom.
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll.abs() > 0.1 {
                self.renderer.camera.zoom(scroll * 0.01);
            }

            // Left click for selection.
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    // Convert to viewport-relative coordinates for picking.
                    let local_pos = pos - rect.min;
                    log::debug!("Click at viewport ({}, {})", local_pos.x, local_pos.y);

                    // TODO: Perform GPU picking readback at this position.
                    // For now, just log the click.
                }
            }

            // Escape cancels selection and overlay.
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.selection = SelectionState::None;
                self.overlay.close();
            }

            // Placeholder: draw a dark background where the 3D viewport will be.
            ui.painter().rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgb(30, 30, 35),
            );

            if self.renderer.mesh.is_none() {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "3D Viewport — describe a part to get started",
                    egui::FontId::proportional(16.0),
                    egui::Color32::from_rgb(80, 80, 90),
                );
            }

            // Show the spatial comment overlay if active.
            let overlay_action = self.overlay.show(ui);
            match overlay_action {
                cadmark_ui::overlay::OverlayAction::Submit(text) => {
                    // TODO: Build real geometry context from current selection.
                    // For now, create a placeholder context.
                    if let SelectionState::Selected(ref _element) = self.selection {
                        // TODO: Resolve element to geometry context via provenance.
                        log::info!("Spatial comment submitted: {text}");
                    }
                    self.overlay.close();
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
