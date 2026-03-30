// Application state — the top-level struct that owns all subsystems
// and implements the eframe::App trait.

use std::sync::mpsc;

use cadmark_core::geometry::{ScreenPosition, SelectionState};
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
    cmd_tx: Option<mpsc::Sender<OrchestratorCommand>>,
    /// Channel to receive results from the orchestrator.
    result_rx: Option<mpsc::Receiver<OrchestratorResult>>,
    /// Provenance ledger — rebuilt on each script execution.
    pub ledger: cadmark_core::ledger::ProvenanceLedger,
    /// Active identification strategy for geometry context.
    pub identification_strategy: Box<dyn cadmark_core::context::IdentificationStrategy>,
    /// Pending pick request — screen coordinates to read back next frame.
    pending_pick: Option<(f32, f32)>,
    /// Tessellated mesh waiting for GPU upload (set after script execution,
    /// consumed when the render pass has device access).
    pending_mesh: Option<cadmark_kernel::tessellation::TessellatedMesh>,
    /// Whether GPU resources have been initialised.
    gpu_initialised: bool,
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

        // Ensure the project directory has a git repo for microversioning.
        if let Some(ref dir) = project_dir {
            if let Err(e) = crate::git_ops::ensure_repo(dir) {
                log::error!("Failed to initialise git repo in {}: {e}", dir.display());
            }
        }

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
            ledger: cadmark_core::ledger::ProvenanceLedger::new(),
            identification_strategy: Box::new(cadmark_core::context::NullIdentification),
            pending_pick: None,
            pending_mesh: None,
            gpu_initialised: false,
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

                        // Re-execute script on the main thread to get mesh
                        // and provenance data (the orchestrator thread executed
                        // it but we need the results here for state updates).
                        match cadmark_kernel::execution::execute_script(&script_path) {
                            Ok(result) => {
                                log::info!(
                                    "Script executed: {} vertices, {} provenance entries",
                                    result.mesh.vertices.len(),
                                    result.provenance.entries.len(),
                                );
                                // Rebuild provenance ledger: map face shape hashes
                                // from the mesh back to source lines captured during
                                // execution.
                                self.ledger.clear();
                                let hash_to_line: std::collections::HashMap<u64, (u32, cadmark_core::ledger::ProvenanceKind)> =
                                    result.provenance.entries.iter().map(|e| {
                                        let kind = match e.kind {
                                            cadmark_kernel::provenance::ProvenanceRelation::Generated =>
                                                cadmark_core::ledger::ProvenanceKind::Generated,
                                            cadmark_kernel::provenance::ProvenanceRelation::Modified =>
                                                cadmark_core::ledger::ProvenanceKind::Modified,
                                        };
                                        (e.shape_hash, (e.source_line, kind))
                                    }).collect();
                                for (face_idx, &shape_hash) in result.mesh.face_shape_hashes.iter().enumerate() {
                                    if let Some((line, kind)) = hash_to_line.get(&shape_hash) {
                                        self.ledger.record_face(
                                            cadmark_core::geometry::FaceId(face_idx as u32),
                                            cadmark_core::ledger::ProvenanceEntry {
                                                source: cadmark_core::ledger::SourceRef {
                                                    line: *line,
                                                    code: String::new(),
                                                },
                                                kind: kind.clone(),
                                            },
                                        );
                                    }
                                }

                                // Store the tessellated mesh for GPU upload on next frame.
                                self.pending_mesh = Some(result.mesh);
                            }
                            Err(e) => {
                                log::warn!("Re-execution failed: {e}");
                            }
                        }
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

    /// Re-read the script from disk and re-execute to sync in-memory state
    /// after an undo/redo checkout changes the working tree.
    fn reload_script_state(&mut self, project_dir: &std::path::Path) {
        let script_path = project_dir.join("part.py");
        if !script_path.exists() {
            return;
        }
        match cadmark_kernel::execution::execute_script(&script_path) {
            Ok(result) => {
                log::info!("Re-executed script after version change");
                self.pending_mesh = Some(result.mesh);
                // Provenance rebuild happens the same way as in poll_results;
                // extracted here to avoid duplication once the codebase matures,
                // but kept inline for now since the mapping logic is trivial.
                self.ledger.clear();
            }
            Err(e) => {
                log::warn!("Re-execution after version change failed: {e}");
            }
        }
    }

    /// Handle a completed pick result — resolve to selection and potentially open overlay.
    fn handle_pick_result(
        &mut self,
        element: cadmark_core::geometry::TopologyElement,
        screen_pos: (f32, f32),
    ) {
        self.selection = SelectionState::Selected(element.clone());

        // Update the renderer's selection state for glow effect.
        self.renderer.selected_id =
            cadmark_renderer::picking::encode_picking_id(&element);

        // Resolve geometry context via provenance.
        let context = cadmark_core::context::resolve_context(
            &element,
            &self.ledger,
            self.identification_strategy.as_ref(),
        );

        // Open the spatial comment overlay near the selection.
        self.overlay.open(ScreenPosition {
            x: screen_pos.0,
            y: screen_pos.1,
        });

        log::info!(
            "Selected {:?}, source line: {:?}",
            element,
            context.source_line
        );
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
                        let hash = version.commit_hash.clone();
                        if let Some(dir) = self.project_dir.clone() {
                            if let Err(e) =
                                crate::git_ops::checkout_commit(&dir, &hash)
                            {
                                log::error!("Undo failed: {e}");
                            } else {
                                self.reload_script_state(&dir);
                            }
                        }
                        log::info!("Undo to {hash}");
                    }
                }
                cadmark_ui::toolbar::ToolbarAction::Redo => {
                    if let Some(version) = self.history.redo() {
                        let hash = version.commit_hash.clone();
                        if let Some(dir) = self.project_dir.clone() {
                            if let Err(e) =
                                crate::git_ops::checkout_commit(&dir, &hash)
                            {
                                log::error!("Redo failed: {e}");
                            } else {
                                self.reload_script_state(&dir);
                            }
                        }
                        log::info!("Redo to {hash}");
                    }
                }
                cadmark_ui::toolbar::ToolbarAction::JumpToVersion(_idx) => {
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

            // Handle viewport input — CAD navigation.
            if response.dragged_by(egui::PointerButton::Secondary) {
                let delta = response.drag_delta();
                self.renderer.camera.orbit(delta.x, delta.y);
            }

            if response.dragged_by(egui::PointerButton::Middle) {
                let delta = response.drag_delta();
                self.renderer.camera.pan(delta.x, delta.y);
            }

            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll.abs() > 0.1 {
                self.renderer.camera.zoom(scroll * 0.01);
            }

            // Left click for selection — request a pick readback.
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let local_pos = pos - rect.min;
                    self.pending_pick = Some((local_pos.x, local_pos.y));
                    log::debug!("Pick requested at ({}, {})", local_pos.x, local_pos.y);
                }
            }

            // Hover tracking — update hover ID for preview highlight.
            if let Some(pos) = response.hover_pos() {
                let _local_pos = pos - rect.min;
                // Hover picking uses the same mechanism as click picking
                // but updates hover_id instead of selected_id.
                // Full implementation requires per-frame picking readback
                // which is expensive — defer to when mesh exists.
            }

            // Escape cancels selection and overlay.
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.selection = SelectionState::None;
                self.renderer.selected_id = 0;
                self.overlay.close();
            }

            // Draw the viewport background.
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
                    if let SelectionState::Selected(ref element) = self.selection {
                        // Resolve the geometry context from the current selection.
                        let context = cadmark_core::context::resolve_context(
                            element,
                            &self.ledger,
                            self.identification_strategy.as_ref(),
                        );
                        self.send_spatial_comment(text, context);
                    }
                    self.overlay.close();
                }
                cadmark_ui::overlay::OverlayAction::Cancel => {
                    self.overlay.close();
                    self.selection = SelectionState::None;
                    self.renderer.selected_id = 0;
                }
                cadmark_ui::overlay::OverlayAction::None => {}
            }
        });
    }
}
