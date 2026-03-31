// Application state — the top-level struct that owns all subsystems
// and implements the eframe::App trait.
//
// The 3D viewport is rendered via egui_wgpu paint callbacks. GPU
// resources (pipelines, picking texture, mesh buffers) live in
// egui_wgpu's callback_resources so the paint callback can access
// them without lifetime gymnastics.

use std::sync::mpsc;

use cadmark_core::geometry::{ScreenPosition, SelectionState};
use cadmark_core::message::Conversation;
use cadmark_core::version::VersionHistory;
use cadmark_renderer::pipeline::Renderer;
use cadmark_ui::chat::ChatPane;
use cadmark_ui::overlay::OverlayState;

use crate::orchestrator::{OrchestratorCommand, OrchestratorResult};

// ── Viewport GPU resources ──────────────────────────────────────────

/// GPU resources for the 3D viewport, stored in egui_wgpu's
/// `callback_resources` so both `prepare()` and `paint()` can reach them.
struct ViewportResources {
    pipelines: cadmark_renderer::pipeline::RenderPipelines,
    picking: cadmark_renderer::picking::PickingPass,
    mesh: Option<cadmark_renderer::mesh::GpuMesh>,
    /// A pick readback was issued last frame and the staging buffer
    /// is ready to map.
    readback_pending: bool,
    /// Decoded pick result from the most recent readback, waiting
    /// for `update()` to consume it.
    pick_result: Option<cadmark_core::geometry::TopologyElement>,
    /// Last-known viewport size in physical pixels — triggers resize
    /// of the picking texture and depth buffer when it changes.
    viewport_size: (u32, u32),
}

/// Per-frame data passed into the egui_wgpu paint callback.
/// Carries everything that changes frame-to-frame (uniforms,
/// whether a pick was requested, viewport dimensions).
struct ViewportCallback {
    mesh_uniforms: cadmark_renderer::pipeline::MeshUniforms,
    simple_uniforms: cadmark_renderer::pipeline::SimpleUniforms,
    /// Pixel coordinates within the viewport to read back for
    /// picking, if the user clicked this frame.
    pick_request: Option<(u32, u32)>,
    /// Viewport size in physical pixels (for resize detection).
    viewport_size: (u32, u32),
}

impl eframe::egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &eframe::egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut eframe::egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = callback_resources.get_mut::<ViewportResources>() else {
            return Vec::new();
        };

        // ── Read back previous frame's pick result ──
        if res.readback_pending {
            res.readback_pending = false;
            let slice = res.picking.staging_buffer.slice(..256);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            slice.map_async(wgpu::MapMode::Read, move |v| {
                let _ = tx.send(v);
            });
            device.poll(wgpu::Maintain::Wait);
            if let Ok(Ok(())) = rx.recv() {
                let data = slice.get_mapped_range();
                res.pick_result =
                    cadmark_renderer::viewport::decode_pick_result(&data);
                drop(data);
                res.picking.staging_buffer.unmap();
            }
        }

        // ── Resize picking texture + depth if viewport changed ──
        let (w, h) = self.viewport_size;
        if w > 0 && h > 0 && (w, h) != res.viewport_size {
            res.picking.resize(device, w, h);
            res.pipelines.resize(device, w, h);
            res.viewport_size = (w, h);
        }

        // ── Write per-frame uniforms ──
        queue.write_buffer(
            &res.pipelines.mesh_uniform_buffer,
            0,
            bytemuck::bytes_of(&self.mesh_uniforms),
        );
        queue.write_buffer(
            &res.pipelines.picking_uniform_buffer,
            0,
            bytemuck::bytes_of(&self.simple_uniforms),
        );
        queue.write_buffer(
            &res.pipelines.wireframe_uniform_buffer,
            0,
            bytemuck::bytes_of(&self.simple_uniforms),
        );

        // ── Offscreen picking pass + optional readback ──
        if let Some(mesh) = &res.mesh {
            // Render colour-ID picking pass to offscreen texture.
            {
                let mut pass =
                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("picking_pass"),
                        color_attachments: &[Some(
                            wgpu::RenderPassColorAttachment {
                                view: &res.picking.texture_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(
                                        wgpu::Color::BLACK,
                                    ),
                                    store: wgpu::StoreOp::Store,
                                },
                            },
                        )],
                        depth_stencil_attachment: Some(
                            wgpu::RenderPassDepthStencilAttachment {
                                view: &res.pipelines.depth_texture,
                                depth_ops: Some(wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(1.0),
                                    store: wgpu::StoreOp::Store,
                                }),
                                stencil_ops: None,
                            },
                        ),
                        ..Default::default()
                    });

                pass.set_pipeline(&res.pipelines.picking_pipeline);
                pass.set_bind_group(
                    0,
                    &res.pipelines.picking_bind_group,
                    &[],
                );
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    mesh.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }

            // ── Offscreen main viewport pass (with depth) ──
            // The egui paint callback's render pass has no depth attachment,
            // so we render the shaded mesh + wireframe here with our own
            // depth texture, then blit the result in paint().
            {
                let mut pass =
                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("viewport_main_pass"),
                        color_attachments: &[Some(
                            wgpu::RenderPassColorAttachment {
                                view: &res.pipelines.viewport_colour_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: 30.0 / 255.0,
                                        g: 30.0 / 255.0,
                                        b: 35.0 / 255.0,
                                        a: 1.0,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            },
                        )],
                        depth_stencil_attachment: Some(
                            wgpu::RenderPassDepthStencilAttachment {
                                view: &res.pipelines.depth_texture,
                                depth_ops: Some(wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(1.0),
                                    store: wgpu::StoreOp::Store,
                                }),
                                stencil_ops: None,
                            },
                        ),
                        ..Default::default()
                    });

                // Shaded mesh pass.
                pass.set_pipeline(&res.pipelines.mesh_pipeline);
                pass.set_bind_group(
                    0,
                    &res.pipelines.mesh_bind_group,
                    &[],
                );
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    mesh.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);

                // Wireframe overlay.
                if mesh.edge_vertex_count > 0 {
                    pass.set_pipeline(&res.pipelines.wireframe_pipeline);
                    pass.set_bind_group(
                        0,
                        &res.pipelines.wireframe_bind_group,
                        &[],
                    );
                    pass.set_vertex_buffer(
                        0,
                        mesh.edge_vertex_buffer.slice(..),
                    );
                    pass.draw(0..mesh.edge_vertex_count, 0..1);
                }
            }

            // Copy a single pixel from the picking texture to the
            // staging buffer so we can map it next frame.
            if let Some((x, y)) = self.pick_request {
                cadmark_renderer::viewport::request_pick_readback(
                    encoder,
                    &res.picking,
                    x,
                    y,
                );
                res.readback_pending = true;
            }
        }

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &eframe::egui_wgpu::CallbackResources,
    ) {
        let Some(res) = callback_resources.get::<ViewportResources>() else {
            return;
        };

        // Blit the offscreen viewport texture (rendered in prepare()
        // with depth testing) onto the egui render pass.
        render_pass.set_pipeline(&res.pipelines.blit_pipeline);
        render_pass.set_bind_group(
            0,
            &res.pipelines.blit_bind_group,
            &[],
        );
        render_pass.draw(0..3, 0..1);
    }
}

// ── Application state ───────────────────────────────────────────────

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
    /// 3D renderer state (camera, selection IDs, style).
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
    /// Tessellated mesh waiting for GPU upload (set after script
    /// execution, consumed in `update()` when the render state is
    /// accessible).
    pending_mesh: Option<cadmark_kernel::tessellation::TessellatedMesh>,
    /// Local click coordinates (relative to viewport rect) for the
    /// pending pick request. Consumed in the same frame to build the
    /// paint callback.
    pending_pick: Option<(f32, f32)>,
    /// Absolute screen position of the in-flight pick, preserved
    /// across frames so `handle_pick_result` can position the overlay
    /// correctly when the readback arrives.
    pick_in_flight: Option<(f32, f32)>,
    /// Whether a mesh has been uploaded to the GPU (for placeholder
    /// text logic — avoids locking the renderer to check).
    has_mesh: bool,
    /// Cloned render state for GPU access from the UI thread.
    wgpu_render_state: Option<eframe::egui_wgpu::RenderState>,
}

impl CadmarkApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
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

        // Initialise GPU resources and insert into callback_resources
        // so the paint callback can reach them.
        let wgpu_render_state = cc.wgpu_render_state.clone();
        if let Some(ref rs) = wgpu_render_state {
            let format = rs.target_format;
            // Start with a reasonable default size — the first frame's
            // prepare() will resize to the actual viewport.
            let (w, h) = (800, 600);
            let pipelines =
                cadmark_renderer::pipeline::RenderPipelines::new(
                    &rs.device, format, w, h,
                );
            let picking =
                cadmark_renderer::picking::PickingPass::new(&rs.device, w, h);

            let resources = ViewportResources {
                pipelines,
                picking,
                mesh: None,
                readback_pending: false,
                pick_result: None,
                viewport_size: (w, h),
            };
            rs.renderer.write().callback_resources.insert(resources);
        }

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
            pending_mesh: None,
            pending_pick: None,
            pick_in_flight: None,
            has_mesh: false,
            wgpu_render_state,
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

                                // Store the tessellated mesh for GPU upload on
                                // the next frame (update() has access to the
                                // wgpu device via the render state).
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
        // Read the current script source to sync the orchestrator.
        let script_source = std::fs::read_to_string(&script_path).ok();


        match cadmark_kernel::execution::execute_script(&script_path) {
            Ok(result) => {
                log::info!("Re-executed script after version change");

                // Sync the orchestrator's cached code so the next AI request
                // sends the correct (post-undo/redo) source.
                if let (Some(tx), Some(code)) = (&self.cmd_tx, script_source) {
                    let _ = tx.send(OrchestratorCommand::UpdateCode(code));
                }

                // Rebuild provenance ledger from the re-executed script
                // (same mapping as poll_results).
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

                self.pending_mesh = Some(result.mesh);
            }
            Err(e) => {
                log::warn!("Re-execution after version change failed: {e}");
            }
        }
    }

    /// Handle a completed pick result — resolve to selection and
    /// potentially open the spatial comment overlay.
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

    // ── GPU helpers called from update() ────────────────────────────

    /// Upload a pending tessellated mesh to GPU buffers.
    fn drain_pending_mesh(&mut self) {
        let mesh = match self.pending_mesh.take() {
            Some(m) => m,
            None => return,
        };
        let Some(rs) = &self.wgpu_render_state else { return };

        let gpu_mesh =
            cadmark_renderer::pipeline::upload_mesh(&rs.device, &mesh);

        let mut renderer = rs.renderer.write();
        if let Some(res) =
            renderer.callback_resources.get_mut::<ViewportResources>()
        {
            res.mesh = Some(gpu_mesh);
        }
        self.has_mesh = true;
    }

    /// Check callback_resources for a decoded pick result from the
    /// previous frame's readback. If one exists, consume it.
    fn consume_pick_result(&mut self) {
        let Some(rs) = self.wgpu_render_state.clone() else { return };

        // Extract the pick result from callback_resources.
        let element = {
            let mut renderer = rs.renderer.write();
            let Some(res) =
                renderer.callback_resources.get_mut::<ViewportResources>()
            else {
                return;
            };
            res.pick_result.take()
        };

        if let Some(element) = element {
            if let Some(screen_pos) = self.pick_in_flight.take() {
                self.handle_pick_result(element, screen_pos);
            }
        } else {
            // Background click (pick ID 0) — clear the in-flight
            // state so we stop requesting repaints.
            self.pick_in_flight.take();
        }
    }
}

impl eframe::App for CadmarkApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll for async results from the orchestrator.
        self.poll_results();

        // Drain pending GPU work before building the frame.
        self.consume_pick_result();
        self.drain_pending_mesh();

        // Request continuous repaints while AI is busy (to poll results)
        // or when a pick readback is in flight.
        if self.ai_busy || self.pick_in_flight.is_some() {
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
                    // Local coordinates feed the GPU picking texture.
                    self.pending_pick = Some((local_pos.x, local_pos.y));
                    // Absolute coordinates position the overlay on result.
                    self.pick_in_flight = Some((pos.x, pos.y));
                    log::debug!(
                        "Pick requested at ({}, {})",
                        local_pos.x,
                        local_pos.y
                    );
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

            // ── Viewport rendering ──────────────────────────────────

            // Background rect — always drawn so the viewport has a
            // consistent dark fill even before any mesh is loaded.
            ui.painter().rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgb(30, 30, 35),
            );

            // Build the paint callback that drives the wgpu renderer.
            let ppp = ctx.pixels_per_point();
            let viewport_size = (
                (rect.width() * ppp) as u32,
                (rect.height() * ppp) as u32,
            );
            let aspect = rect.width() / rect.height().max(1.0);

            // Consume the pending pick (local coords → pixel coords).
            let pick_request = self.pending_pick.take().map(|(x, y)| {
                ((x * ppp) as u32, (y * ppp) as u32)
            });

            let callback = eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                ViewportCallback {
                    mesh_uniforms: self.renderer.mesh_uniforms(aspect),
                    simple_uniforms: self.renderer.simple_uniforms(aspect),
                    pick_request,
                    viewport_size,
                },
            );
            ui.painter().add(callback);

            // Placeholder text when no mesh is loaded yet.
            if !self.has_mesh {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "3D Viewport \u{2014} describe a part to get started",
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
