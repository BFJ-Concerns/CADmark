// Application state — the top-level struct that owns all subsystems
// and implements the eframe::App trait.
//
// The 3D viewport is rendered via egui_wgpu paint callbacks. GPU
// resources (pipelines, picking texture, mesh buffers) live in
// egui_wgpu's callback_resources so the paint callback can access
// them without lifetime gymnastics.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime};

use cadmark_core::context::{IdentificationStrategy, MeasuredIdentification};
use cadmark_core::export::ExportFormat;
use cadmark_core::geometry::{ModelSummary, ScreenPosition, SelectionState};
use cadmark_core::ledger::ProvenanceLedger;
use cadmark_core::message::{Conversation, Message};
use cadmark_core::version::VersionHistory;
use cadmark_kernel::execution::ExecutionResult;
use cadmark_kernel::export::ModelHandle;
use cadmark_renderer::camera::Bounds3;
use cadmark_renderer::pipeline::Renderer;
use cadmark_ui::chat::{ChatActivity, ChatPane};
use cadmark_ui::code_panel::{CodePanel, CodePanelAction, CodeView};
use cadmark_ui::overlay::OverlayState;
use cadmark_ui::status::{Status, StatusView};
use cadmark_ui::toolbar::{ToolbarAction, ToolbarState};
use cadmark_ui::version_dialog::{VersionDialog, VersionDialogAction};

use crate::orchestrator::{ModelOrigin, OrchestratorCommand, OrchestratorResult};
use crate::user_settings::RecentProjects;

// ── Viewport GPU resources ──────────────────────────────────────────

/// GPU resources for the 3D viewport, stored in egui_wgpu's
/// `callback_resources` so both `prepare()` and `paint()` can reach them.
struct ViewportResources {
    pipelines: cadmark_renderer::pipeline::RenderPipelines,
    picking: cadmark_renderer::picking::PickingPass,
    mesh: Option<cadmark_renderer::mesh::GpuMesh>,
    /// Pick attempt submitted through the independent readback encoder. Its
    /// marker proves whether those commands completed before bytes are trusted.
    pick_attempt: Option<PickAttempt>,
    submission_marker_source: wgpu::Buffer,
    submission_marker_staging: wgpu::Buffer,
    next_submission_token: u32,
    retry_pick: Option<(u32, u32)>,
    /// Decoded pick result from the most recent readback, waiting
    /// for `update()` to consume it.
    pick_result: Option<CompletedPick>,
    /// Per-frame hover picking. Unlike a click, a hover readback is never
    /// waited for: the mapped result is collected on a later frame, and
    /// a frame whose readback is still in flight issues no new one.
    hover: HoverPick,
    /// Last-known viewport size in physical pixels — triggers resize
    /// of the picking texture and depth buffer when it changes.
    viewport_size: (u32, u32),
}

enum CompletedPick {
    Hit(cadmark_core::geometry::TopologyElement),
    Background,
    ReadbackFailed,
}

/// Where a buffer-mapping callback leaves its outcome for a later frame.
type MapStatus = Arc<Mutex<Option<Result<(), wgpu::BufferAsyncError>>>>;

/// Asynchronous readback of the element under the cursor.
struct HoverPick {
    /// One-pixel staging buffer, separate from the click path's so the two
    /// readbacks never contend for a mapping.
    staging: wgpu::Buffer,
    /// Mapping in progress; the callback stores its status here and a later
    /// frame's poll reads it.
    in_flight: Option<MapStatus>,
    /// Picking ID under the cursor from the most recent completed readback
    /// (0 = background), waiting for `update()` to consume it.
    result: Option<u32>,
}

impl HoverPick {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            staging: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hover_pick_staging"),
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            in_flight: None,
            result: None,
        }
    }

    /// Collect a finished readback without blocking. A mapping that has not
    /// completed stays in flight; a failed one is dropped, and the next
    /// frame simply picks again.
    fn collect(&mut self, device: &wgpu::Device) {
        let Some(status) = &self.in_flight else {
            return;
        };
        let _ = device.poll(wgpu::Maintain::Poll);
        let status = status.lock().map(|mut slot| slot.take()).unwrap_or(None);
        match status {
            Some(Ok(())) => {
                let data = self.staging.slice(..4).get_mapped_range();
                let pixel = [data[0], data[1], data[2], data[3]];
                drop(data);
                self.staging.unmap();
                self.result = Some(cadmark_renderer::picking::colour_to_id(pixel));
                self.in_flight = None;
            }
            Some(Err(error)) => {
                log::debug!("Hover readback failed: {error}");
                self.in_flight = None;
            }
            None => {}
        }
    }

    /// Begin mapping the staging buffer once the copy into it is submitted.
    fn begin_readback(&mut self) {
        let slot = Arc::new(Mutex::new(None));
        let writer = Arc::clone(&slot);
        self.staging
            .slice(..4)
            .map_async(wgpu::MapMode::Read, move |status| {
                if let Ok(mut slot) = writer.lock() {
                    *slot = Some(status);
                }
            });
        self.in_flight = Some(slot);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PickAttempt {
    pixel: (u32, u32),
    submission_token: u32,
}

#[derive(Debug, PartialEq, Eq)]
enum PickSubmissionDecision {
    ReadPick,
    Retry((u32, u32)),
}

fn pick_submission_decision(attempt: PickAttempt, marker_bytes: [u8; 4]) -> PickSubmissionDecision {
    if u32::from_le_bytes(marker_bytes) == attempt.submission_token {
        PickSubmissionDecision::ReadPick
    } else {
        PickSubmissionDecision::Retry(attempt.pixel)
    }
}

#[derive(Debug, PartialEq)]
enum PickTransition {
    Waiting,
    Hit(cadmark_core::geometry::TopologyElement, (f32, f32)),
    Background,
    ReadbackFailed,
}

fn completed_pick_transition(
    completed: Option<CompletedPick>,
    pick_in_flight: &mut Option<(f32, f32)>,
) -> PickTransition {
    match completed {
        None => PickTransition::Waiting,
        Some(CompletedPick::Hit(element)) => match pick_in_flight.take() {
            Some(screen_pos) => PickTransition::Hit(element, screen_pos),
            None => PickTransition::Background,
        },
        Some(CompletedPick::Background) => {
            pick_in_flight.take();
            PickTransition::Background
        }
        Some(CompletedPick::ReadbackFailed) => {
            pick_in_flight.take();
            PickTransition::ReadbackFailed
        }
    }
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
    /// Pixel coordinates under the cursor, if it is over the viewport.
    hover_request: Option<(u32, u32)>,
    /// Viewport size in physical pixels (for resize detection).
    viewport_size: (u32, u32),
    /// Background colour in the offscreen target's own colour space.
    clear_colour: wgpu::Color,
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

        // Confirm that the independent picking encoder completed before
        // trusting readback bytes. A stale marker retries the exact pixel.
        if let Some(attempt) = res.pick_attempt.take() {
            let slice = res.submission_marker_staging.slice(..4);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            slice.map_async(wgpu::MapMode::Read, move |status| {
                let _ = tx.send(status);
            });
            device.poll(wgpu::Maintain::Wait);
            match rx.recv() {
                Ok(Ok(())) => {
                    let data = slice.get_mapped_range();
                    let marker_bytes = [data[0], data[1], data[2], data[3]];
                    drop(data);
                    res.submission_marker_staging.unmap();

                    match pick_submission_decision(attempt, marker_bytes) {
                        PickSubmissionDecision::ReadPick => {
                            let slice = res.picking.staging_buffer.slice(..256);
                            let (tx, rx) = std::sync::mpsc::sync_channel(1);
                            slice.map_async(wgpu::MapMode::Read, move |status| {
                                let _ = tx.send(status);
                            });
                            device.poll(wgpu::Maintain::Wait);
                            match rx.recv() {
                                Ok(Ok(())) => {
                                    let data = slice.get_mapped_range();
                                    res.pick_result = Some(
                                        match cadmark_renderer::viewport::decode_pick_result(&data)
                                        {
                                            Some(element) => CompletedPick::Hit(element),
                                            None => CompletedPick::Background,
                                        },
                                    );
                                    drop(data);
                                    res.picking.staging_buffer.unmap();
                                }
                                _ => {
                                    res.pick_result = Some(CompletedPick::ReadbackFailed);
                                }
                            }
                        }
                        PickSubmissionDecision::Retry(pixel) => {
                            log::debug!(
                                "Pick submission was dropped; retrying at ({}, {})",
                                pixel.0,
                                pixel.1
                            );
                            res.retry_pick = Some(pixel);
                        }
                    }
                }
                _ => {
                    res.pick_result = Some(CompletedPick::ReadbackFailed);
                }
            }
        }

        // ── Resize picking texture + depth if viewport changed ──
        let (w, h) = self.viewport_size;
        if w > 0 && h > 0 && (w, h) != res.viewport_size {
            res.picking.resize(device, w, h);
            res.pipelines.resize(device, w, h);
            res.viewport_size = (w, h);
        }

        res.hover.collect(device);

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

        // ── Offscreen rendering + optional pick readback ──
        if let Some(mesh) = &res.mesh {
            if self.pick_request.is_some() {
                res.retry_pick = None;
            }
            if let Some((x, y)) = self.pick_request.or_else(|| res.retry_pick.take()) {
                // Submit picking independently. eframe acquires the surface
                // after prepare(); an Outdated surface returns early and drops
                // its shared encoder, but must not drop user selection work.
                let mut pick_encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("pick_readback_encoder"),
                    });
                encode_picking_passes(&mut pick_encoder, &res.pipelines, &res.picking, mesh);

                let submission_token = res.next_submission_token;
                res.next_submission_token = res.next_submission_token.wrapping_add(1).max(1);
                queue.write_buffer(
                    &res.submission_marker_source,
                    0,
                    &submission_token.to_le_bytes(),
                );
                cadmark_renderer::viewport::request_pick_readback(
                    &mut pick_encoder,
                    &res.picking,
                    x,
                    y,
                );
                pick_encoder.copy_buffer_to_buffer(
                    &res.submission_marker_source,
                    0,
                    &res.submission_marker_staging,
                    0,
                    4,
                );
                queue.submit(std::iter::once(pick_encoder.finish()));
                res.pick_attempt = Some(PickAttempt {
                    pixel: (x, y),
                    submission_token,
                });
            } else if let Some((x, y)) = self.hover_request
                && res.hover.in_flight.is_none()
            {
                // Hover shares the click path's picking texture but not its
                // wait: the readback is collected on a later frame.
                let mut hover_encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("hover_readback_encoder"),
                    });
                encode_picking_passes(&mut hover_encoder, &res.pipelines, &res.picking, mesh);
                cadmark_renderer::viewport::copy_pick_pixel(
                    &mut hover_encoder,
                    &res.picking,
                    &res.hover.staging,
                    x,
                    y,
                );
                queue.submit(std::iter::once(hover_encoder.finish()));
                res.hover.begin_readback();
            }
        }

        // ── Offscreen main viewport pass (with depth) ──
        // The egui paint callback's render pass has no depth attachment,
        // so we render the shaded mesh + wireframe here with our own
        // depth texture, then blit the result in paint(). The pass always
        // runs so that clearing the model leaves no stale render behind.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport_main_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &res.pipelines.viewport_colour_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_colour),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &res.pipelines.depth_texture,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            if let Some(mesh) = &res.mesh {
                // Shaded mesh pass.
                pass.set_pipeline(&res.pipelines.mesh_pipeline);
                pass.set_bind_group(0, &res.pipelines.mesh_bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);

                // Wireframe overlay.
                if mesh.edge_vertex_count > 0 {
                    pass.set_pipeline(&res.pipelines.wireframe_pipeline);
                    pass.set_bind_group(0, &res.pipelines.mesh_bind_group, &[]);
                    pass.set_vertex_buffer(0, mesh.edge_vertex_buffer.slice(..));
                    pass.draw(0..mesh.edge_vertex_count, 0..1);
                }
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
        render_pass.set_bind_group(0, &res.pipelines.blit_bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

/// Render every pickable element into the colour-ID texture: faces first,
/// then edges drawn over them so a cursor on an edge picks the edge.
fn encode_picking_passes(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &cadmark_renderer::pipeline::RenderPipelines,
    picking: &cadmark_renderer::picking::PickingPass,
    mesh: &cadmark_renderer::mesh::GpuMesh,
) {
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("picking_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &picking.texture_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Every channel must clear to zero: the texture is
                    // Rgba8Uint, so an alpha of 1.0 would land as the byte
                    // 1 and decode as a vertex rather than the background.
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &pipelines.depth_texture,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        pass.set_pipeline(&pipelines.picking_pipeline);
        pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }

    if mesh.edge_vertex_count > 0 {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("edge_picking_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &picking.texture_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &pipelines.depth_texture,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        pass.set_pipeline(&pipelines.edge_picking_pipeline);
        pass.set_bind_group(0, &pipelines.picking_bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.edge_vertex_buffer.slice(..));
        pass.draw(0..mesh.edge_vertex_count, 0..1);
    }
}

// ── Application state ───────────────────────────────────────────────

const SCRIPT_FILENAME: &str = "part.py";

/// How often the script on disk is compared with the model on screen.
const SCRIPT_WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// What the worker thread is doing, for the status bar and chat spinner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    /// Waiting on the AI, then executing its code.
    Generating,
    /// Executing the script on disk.
    Building,
}

impl Busy {
    fn label(self) -> &'static str {
        match self {
            Self::Generating => "Generating\u{2026}",
            Self::Building => "Building model\u{2026}",
        }
    }

    fn chat_activity(self) -> ChatActivity {
        match self {
            Self::Generating => ChatActivity::Generating,
            Self::Building => ChatActivity::Building,
        }
    }
}

/// The model on screen: what the UI keeps from the last successful
/// execution besides the mesh, which lives on the GPU.
struct LoadedModel {
    summary: ModelSummary,
    bounds: Option<Bounds3>,
    handle: ModelHandle,
}

/// Outcome of a background export, delivered to the UI thread.
struct ExportOutcome {
    format: ExportFormat,
    result: Result<PathBuf, String>,
}

/// The chat line for an accepted edit: the AI's summary, then what measurably
/// changed so an edit that did more than asked is visible at once.
fn edit_chat_message(
    response_message: &str,
    before: Option<&ModelSummary>,
    after: &ModelSummary,
) -> String {
    let mut message = response_message.to_string();
    match before {
        Some(before) => match after.describe_change_from(before) {
            Some(change) => message.push_str(&format!("\n\nModel change: {change}.")),
            None => message.push_str("\n\nModel unchanged: same volume, size and face count."),
        },
        None => message.push_str(&format!("\n\nModel: {}.", after.describe())),
    }
    message
}

/// The offscreen target's clear colour for the theme's viewport background.
/// An sRGB target stores linear values and encodes on write; any other
/// target stores what it is given and the shader encodes its own output.
fn viewport_clear_colour(target_is_srgb: bool) -> wgpu::Color {
    let channel = |value: u8| {
        let gamma = f64::from(value) / 255.0;
        if target_is_srgb {
            if gamma <= 0.04045 {
                gamma / 12.92
            } else {
                ((gamma + 0.055) / 1.055).powf(2.4)
            }
        } else {
            gamma
        }
    };
    let colour = cadmark_ui::theme::VIEWPORT;
    wgpu::Color {
        r: channel(colour.r()),
        g: channel(colour.g()),
        b: channel(colour.b()),
        a: 1.0,
    }
}

/// Everything that is replaced when a different project folder is opened.
struct ProjectSession {
    project_dir: PathBuf,
    cmd_tx: mpsc::Sender<OrchestratorCommand>,
    result_rx: mpsc::Receiver<OrchestratorResult<ExecutionResult>>,
    history: VersionHistory,
    conversation: Conversation,
    ai_model: Option<String>,
}

/// Prepare a project folder and start a worker for it.
fn start_project(project_dir: PathBuf) -> ProjectSession {
    let project_dir = project_dir.canonicalize().unwrap_or(project_dir);

    // Ensure the project directory has a git repo for microversioning.
    if let Err(e) = crate::git_ops::ensure_repo(&project_dir) {
        log::error!(
            "Failed to initialise git repo in {}: {e}",
            project_dir.display()
        );
    }

    let mut conversation = Conversation::new();
    conversation.push(Message::notice(
        "Describe what you'd like to build, or click a face, edge or vertex of the \
         model to comment on it. Every accepted edit is saved to part.py in the \
         project folder and recorded as a design step.",
    ));

    let services = crate::config::load_ai_services(&project_dir).map_err(|error| {
        let reason = error.to_string();
        log::warn!("{reason}");
        conversation.push(Message::notice(format!(
            "AI is unavailable: {reason}. The model still loads, and you can edit \
             {SCRIPT_FILENAME} by hand and press Rebuild."
        )));
        reason
    });
    let ai_model = services
        .as_ref()
        .ok()
        .map(|services| services.model_name().to_string());
    let (cmd_tx, result_rx) = crate::orchestrator::spawn_orchestrator(
        project_dir.clone(),
        SCRIPT_FILENAME.to_string(),
        services,
    );

    let history = match crate::git_ops::list_microversions(&project_dir, 100) {
        Ok(versions) => VersionHistory::from_versions(versions),
        Err(e) => {
            log::warn!("Failed to load microversion history: {e}");
            VersionHistory::new()
        }
    };

    ProjectSession {
        project_dir,
        cmd_tx,
        result_rx,
        history,
        conversation,
        ai_model,
    }
}

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
    pub project_dir: PathBuf,
    /// What the worker is doing, if anything.
    busy: Option<Busy>,
    /// Channel to send commands to the orchestrator.
    cmd_tx: mpsc::Sender<OrchestratorCommand>,
    /// Channel to receive results from the orchestrator.
    result_rx: mpsc::Receiver<OrchestratorResult<ExecutionResult>>,
    /// Background exports report here.
    export_tx: mpsc::Sender<ExportOutcome>,
    export_rx: mpsc::Receiver<ExportOutcome>,
    exports_in_flight: usize,
    /// A folder picker running on its own thread reports here.
    folder_pick_rx: Option<mpsc::Receiver<Option<PathBuf>>>,
    /// Provenance ledger — rebuilt on each script execution.
    pub ledger: ProvenanceLedger,
    /// Active identification strategy for geometry context. Rebuilt from the
    /// measured geometry of each executed model.
    pub identification_strategy: Box<dyn IdentificationStrategy>,
    /// The model on screen, if a script has executed successfully.
    model: Option<LoadedModel>,
    /// The source that produced the model on screen.
    script_source: Option<String>,
    /// Whether the script exists on disk, whether or not it runs.
    has_script: bool,
    /// Modification time of the script when the model was last built, and
    /// when it was last compared with disk.
    script_mtime: Option<SystemTime>,
    script_checked_at: Instant,
    /// Whether the script on disk differs from the model on screen.
    script_modified_on_disk: bool,
    /// Code panel state and visibility.
    code_panel: CodePanel,
    code_visible: bool,
    /// Source line of the selected element, when its provenance is known.
    highlighted_line: Option<u32>,
    /// The version-naming dialog.
    version_dialog: VersionDialog,
    /// Recently opened project folders and where they are stored.
    recent_projects: RecentProjects,
    recent_projects_path: Option<PathBuf>,
    /// The AI model in use, for the toolbar badge.
    ai_model: Option<String>,
    /// Tessellated mesh waiting for GPU upload (set after script
    /// execution, consumed in `update()` when the render state is
    /// accessible).
    pending_mesh: Option<cadmark_kernel::tessellation::TessellatedMesh>,
    /// Bounds to frame once the viewport aspect ratio is known.
    pending_camera_bounds: Option<Bounds3>,
    /// Local click coordinates (relative to viewport rect) for the
    /// pending pick request. Consumed in the same frame to build the
    /// paint callback.
    pending_pick: Option<(f32, f32)>,
    /// Absolute screen position of the in-flight pick, preserved
    /// across frames so `handle_pick_result` can position the overlay
    /// correctly when the readback arrives.
    pick_in_flight: Option<(f32, f32)>,
    /// Whether a hover readback was pending at the start of this frame, so
    /// the frame that collects it is scheduled.
    hover_readback_pending: bool,
    /// The cursor pixel and camera the last hover pick was issued for. A
    /// frame that changes neither issues no new pick, so an idle cursor
    /// costs nothing.
    last_hover_probe: Option<((u32, u32), cadmark_renderer::camera::Camera)>,
    /// Whether a mesh has been uploaded to the GPU (for placeholder
    /// text logic — avoids locking the renderer to check).
    has_mesh: bool,
    /// Cloned render state for GPU access from the UI thread.
    wgpu_render_state: Option<eframe::egui_wgpu::RenderState>,
    /// Last execution status — shown on the viewport and in the status
    /// bar so the user can see what went wrong without checking logs.
    status: Option<Status>,
}

impl CadmarkApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cadmark_ui::theme::apply(&cc.egui_ctx);

        // If a project dir was passed as a CLI argument, use it;
        // otherwise default to the current working directory.
        let project_dir = std::env::args()
            .nth(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().expect("failed to read current directory"));

        // Activate the Python venv so the embedded interpreter can
        // find build123d and other dependencies.
        if let Err(e) = cadmark_kernel::execution::discover_and_activate_venv() {
            log::error!("Failed to activate Python venv: {e}");
        }

        let session = start_project(project_dir);

        // Initialise GPU resources and insert into callback_resources
        // so the paint callback can reach them.
        let wgpu_render_state = cc.wgpu_render_state.clone();
        if let Some(ref rs) = wgpu_render_state {
            let format = rs.target_format;
            // Start with a reasonable default size — the first frame's
            // prepare() will resize to the actual viewport.
            let (w, h) = (800, 600);
            let pipelines =
                cadmark_renderer::pipeline::RenderPipelines::new(&rs.device, format, w, h);
            let picking = cadmark_renderer::picking::PickingPass::new(&rs.device, w, h);
            let hover = HoverPick::new(&rs.device);
            let submission_marker_source = rs.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pick_submission_marker_source"),
                size: 4,
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let submission_marker_staging = rs.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pick_submission_marker_staging"),
                size: 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });

            let resources = ViewportResources {
                pipelines,
                picking,
                mesh: None,
                pick_attempt: None,
                submission_marker_source,
                submission_marker_staging,
                next_submission_token: 1,
                retry_pick: None,
                pick_result: None,
                hover,
                viewport_size: (w, h),
            };
            rs.renderer.write().callback_resources.insert(resources);
        }

        let recent_projects_path = RecentProjects::default_path();
        let mut recent_projects = recent_projects_path
            .as_deref()
            .map(RecentProjects::load)
            .unwrap_or_default();
        recent_projects.remember(&session.project_dir);
        if let Some(path) = &recent_projects_path
            && let Err(error) = recent_projects.save(path)
        {
            log::warn!("Could not save the recent-projects list: {error}");
        }

        let (export_tx, export_rx) = mpsc::channel();
        let mut app = Self {
            conversation: session.conversation,
            chat: ChatPane::new(),
            overlay: OverlayState::default(),
            history: session.history,
            renderer: Renderer::new(),
            selection: SelectionState::None,
            project_dir: session.project_dir,
            busy: None,
            cmd_tx: session.cmd_tx,
            result_rx: session.result_rx,
            export_tx,
            export_rx,
            exports_in_flight: 0,
            folder_pick_rx: None,
            ledger: ProvenanceLedger::new(),
            identification_strategy: Box::new(cadmark_core::context::NullIdentification),
            model: None,
            script_source: None,
            has_script: false,
            script_mtime: None,
            script_checked_at: Instant::now(),
            script_modified_on_disk: false,
            code_panel: CodePanel::default(),
            code_visible: false,
            highlighted_line: None,
            version_dialog: VersionDialog::default(),
            recent_projects,
            recent_projects_path,
            ai_model: session.ai_model,
            pending_mesh: None,
            pending_camera_bounds: None,
            pending_pick: None,
            pick_in_flight: None,
            hover_readback_pending: false,
            last_hover_probe: None,
            has_mesh: false,
            wgpu_render_state,
            status: None,
        };
        app.renderer.target_is_srgb = app
            .wgpu_render_state
            .as_ref()
            .is_some_and(|rs| rs.target_format.is_srgb());
        app.chat.ai_available = app.ai_model.is_some();
        app.apply_window_title(&cc.egui_ctx);
        app.request_reload();
        app
    }

    fn script_path(&self) -> PathBuf {
        self.project_dir.join(SCRIPT_FILENAME)
    }

    fn apply_window_title(&self, ctx: &egui::Context) {
        let name = cadmark_ui::toolbar::project_display_name(&self.project_dir);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{name} \u{2014} CADmark"
        )));
    }

    /// Switch to another project folder: a new worker, history and
    /// conversation, with the viewport cleared until its script has run.
    fn open_project(&mut self, ctx: &egui::Context, project_dir: PathBuf) {
        if self.busy.is_some() {
            return;
        }
        let session = start_project(project_dir);
        self.project_dir = session.project_dir;
        self.cmd_tx = session.cmd_tx;
        self.result_rx = session.result_rx;
        self.history = session.history;
        self.conversation = session.conversation;
        self.ai_model = session.ai_model;
        self.chat = ChatPane::new();
        self.chat.ai_available = self.ai_model.is_some();
        self.script_source = None;
        self.has_script = false;
        self.script_mtime = None;
        self.script_modified_on_disk = false;
        self.status = None;
        self.clear_loaded_model();
        self.renderer.camera = cadmark_renderer::camera::Camera::default();

        self.recent_projects.remember(&self.project_dir);
        if let Some(path) = &self.recent_projects_path
            && let Err(error) = self.recent_projects.save(path)
        {
            log::warn!("Could not save the recent-projects list: {error}");
        }
        self.apply_window_title(ctx);
        self.request_reload();
    }

    /// Show the system folder picker on its own thread; the choice is
    /// collected in `poll_results`.
    fn pick_project_folder(&mut self) {
        if self.folder_pick_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let start_in = self
            .project_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.project_dir.clone());
        std::thread::Builder::new()
            .name("cadmark-folder-picker".into())
            .spawn(move || {
                let choice = rfd::FileDialog::new()
                    .set_title("Open a CADmark project folder")
                    .set_directory(start_in)
                    .pick_folder();
                let _ = tx.send(choice);
            })
            .expect("failed to spawn the folder picker thread");
        self.folder_pick_rx = Some(rx);
    }

    /// Hand a command to the worker and record what it is now doing.
    fn dispatch(&mut self, command: OrchestratorCommand, busy: Busy) {
        if self.cmd_tx.send(command).is_ok() {
            self.busy = Some(busy);
        } else {
            self.status = Some(Status::error(
                "The modelling worker has stopped; restart CADmark",
            ));
        }
    }

    /// Ask the worker to re-read and execute the script on disk.
    fn request_reload(&mut self) {
        self.dispatch(OrchestratorCommand::Reload, Busy::Building);
    }

    /// Send a chat message to the AI backend.
    fn send_chat_message(&mut self, message: String) {
        self.conversation.push(Message::user_chat(&message));
        self.dispatch(OrchestratorCommand::ChatMessage(message), Busy::Generating);
    }

    /// Send a spatial comment to the AI backend.
    fn send_spatial_comment(
        &mut self,
        text: String,
        context: cadmark_core::geometry::GeometryContext,
    ) {
        let message = Message::spatial_comment(&text, context.clone());
        let id = message.id;
        self.conversation.push(message);
        self.dispatch(
            OrchestratorCommand::SpatialComment { id, text, context },
            Busy::Generating,
        );
    }

    /// Poll for worker results (non-blocking).
    fn poll_results(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.result_rx.try_recv() {
            self.busy = None;
            match result {
                OrchestratorResult::ModelReady {
                    model,
                    origin,
                    source,
                } => {
                    self.install_model(model, origin, source);
                }
                OrchestratorResult::ExecutionFailed { ai_message, error } => {
                    self.conversation.push(Message::ai_response(ai_message));
                    self.conversation.push(Message::error_notice(format!(
                        "That code failed to run, so the previous model was kept.\n\n{error}"
                    )));
                    self.record_script_state();
                }
                OrchestratorResult::NoScript => {
                    self.clear_loaded_model();
                    self.script_source = None;
                    self.has_script = false;
                    self.script_mtime = None;
                    self.script_modified_on_disk = false;
                    self.status = Some(Status::info(format!(
                        "No {SCRIPT_FILENAME} yet \u{2014} describe a part to get started"
                    )));
                }
                OrchestratorResult::ReloadFailed { error } => {
                    log::error!("Script execution failed: {error}");
                    self.clear_loaded_model();
                    self.has_script = true;
                    self.record_script_state();
                    self.status = Some(Status::error(format!("Execution error: {error}")));
                    self.conversation.push(Message::error_notice(format!(
                        "{SCRIPT_FILENAME} failed to run.\n\n{error}"
                    )));
                }
                OrchestratorResult::BackendError(error) => {
                    self.conversation
                        .push(Message::error_notice(format!("AI error: {error}")));
                }
            }
        }

        while let Ok(outcome) = self.export_rx.try_recv() {
            self.exports_in_flight = self.exports_in_flight.saturating_sub(1);
            self.status = Some(match outcome.result {
                Ok(path) => Status::info(format!(
                    "Exported {} to {}",
                    outcome.format.label(),
                    path.display()
                )),
                Err(error) => {
                    Status::error(format!("{} export failed: {error}", outcome.format.label()))
                }
            });
        }

        if let Some(rx) = &self.folder_pick_rx {
            match rx.try_recv() {
                Ok(Some(folder)) => {
                    self.folder_pick_rx = None;
                    self.open_project(ctx, folder);
                }
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                    self.folder_pick_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    /// Remember the script's modification time so later edits on disk can
    /// be noticed.
    fn record_script_state(&mut self) {
        self.script_mtime = std::fs::metadata(self.script_path())
            .and_then(|metadata| metadata.modified())
            .ok();
        self.script_modified_on_disk = false;
        self.script_checked_at = Instant::now();
    }

    /// Compare the script on disk with the model on screen, at most once
    /// per watch interval.
    fn watch_script(&mut self) {
        if !self.has_script || self.script_checked_at.elapsed() < SCRIPT_WATCH_INTERVAL {
            return;
        }
        self.script_checked_at = Instant::now();
        let on_disk = std::fs::metadata(self.script_path())
            .and_then(|metadata| metadata.modified())
            .ok();
        self.script_modified_on_disk = on_disk != self.script_mtime;
    }

    /// Take a freshly executed model on screen.
    fn install_model(&mut self, result: ExecutionResult, origin: ModelOrigin, source: String) {
        let previous_summary = self.model.as_ref().map(|model| model.summary.clone());
        let first_model = self.model.is_none();

        if let ModelOrigin::Edit(edit) = origin {
            if let Some(id) = edit.applied_spatial_message_id {
                self.conversation.mark_spatial_applied(id);
            }
            self.conversation
                .push(Message::ai_response(edit_chat_message(
                    &edit.response.message,
                    previous_summary.as_ref(),
                    &result.summary,
                )));

            match crate::git_ops::create_microversion(
                &self.project_dir,
                &edit.response.summary,
                &edit.trigger_message,
                edit.script_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(SCRIPT_FILENAME),
            ) {
                Ok(version) => self.history.push(version),
                Err(e) => log::warn!("Failed to create microversion: {e}"),
            }
        }

        log::info!(
            "Model ready: {} vertices, {} provenance entries",
            result.mesh.vertices.len(),
            result.ledger.len(),
        );
        let mut status = format!("Built {SCRIPT_FILENAME}");
        let untraced = result.ledger.untraced_count();
        if untraced > 0 {
            status.push_str(&format!(
                " ({untraced} elements have no traceable source line)"
            ));
        }
        self.status = Some(Status::info(status));

        // Picking IDs belong to the model they were assigned for.
        self.clear_selection();
        self.ledger = result.ledger;
        self.identification_strategy = Box::new(MeasuredIdentification {
            descriptors: result.descriptors,
        });
        let bounds = Bounds3::from_positions(result.mesh.vertices.iter().map(|v| v.position));
        // Frame the first model; later edits keep the user's view.
        if first_model {
            self.pending_camera_bounds = bounds;
        }
        self.model = Some(LoadedModel {
            summary: result.summary,
            bounds,
            handle: result.model,
        });
        self.pending_mesh = Some(result.mesh);
        self.script_source = Some(source);
        self.has_script = true;
        self.record_script_state();
    }

    fn clear_selection(&mut self) {
        self.selection = SelectionState::None;
        self.renderer.selected_id = 0;
        self.renderer.hover_id = 0;
        self.highlighted_line = None;
        self.overlay.close();
    }

    /// Clear the currently loaded model and any selection/picking state so the
    /// viewport cannot show stale geometry after a reload failure.
    fn clear_loaded_model(&mut self) {
        self.pending_mesh = None;
        self.pending_camera_bounds = None;
        self.ledger.clear();
        self.identification_strategy = Box::new(cadmark_core::context::NullIdentification);
        self.model = None;
        self.has_mesh = false;
        self.pending_pick = None;
        self.pick_in_flight = None;
        self.clear_selection();

        if let Some(rs) = &self.wgpu_render_state {
            let mut renderer = rs.renderer.write();
            if let Some(res) = renderer.callback_resources.get_mut::<ViewportResources>() {
                res.mesh = None;
                res.pick_result = None;
                res.pick_attempt = None;
                res.retry_pick = None;
                res.hover.result = None;
            }
        }
    }

    /// Check out a design step and rebuild the model from it.
    fn restore_version(&mut self, commit_hash: String) {
        match crate::git_ops::checkout_commit(&self.project_dir, &commit_hash) {
            Ok(()) => {
                log::info!("Restored design step {commit_hash}");
                self.request_reload();
            }
            Err(e) => {
                log::error!("Restoring {commit_hash} failed: {e}");
                self.status = Some(Status::error(format!(
                    "Could not restore that design step: {e}"
                )));
            }
        }
    }

    /// Record the current script as a named version.
    fn save_named_version(&mut self, name: String) {
        match crate::git_ops::create_snapshot(&self.project_dir, &name, SCRIPT_FILENAME) {
            Ok(version) => {
                self.history.push(version);
                self.status = Some(Status::info(format!(
                    "Saved version \u{201C}{name}\u{201D}"
                )));
            }
            Err(e) => {
                log::error!("Naming a version failed: {e}");
                self.status = Some(Status::error(format!("Could not save the version: {e}")));
            }
        }
    }

    /// Write the current model next to the script, on a background thread.
    fn export(&mut self, format: ExportFormat) {
        let Some(model) = &self.model else {
            return;
        };
        let handle = model.handle.clone();
        let path = self
            .project_dir
            .join(format!("part.{}", format.extension()));
        let tx = self.export_tx.clone();
        self.exports_in_flight += 1;
        self.status = Some(Status::info(format!(
            "Exporting {}\u{2026}",
            format.label()
        )));
        std::thread::spawn(move || {
            let result = cadmark_kernel::export::export_model(&handle, format, &path)
                .map(|()| path)
                .map_err(|error| error.to_string());
            let _ = tx.send(ExportOutcome { format, result });
        });
    }

    /// Open a path with the system's default handler, reporting failure in
    /// the status bar.
    fn open_externally(&mut self, path: &Path, what: &str) {
        if let Err(error) = open::that_detached(path) {
            self.status = Some(Status::error(format!("Could not open {what}: {error}")));
        }
    }

    /// Handle a completed pick result — resolve to selection and open the
    /// spatial comment overlay.
    fn handle_pick_result(
        &mut self,
        element: cadmark_core::geometry::TopologyElement,
        screen_pos: (f32, f32),
    ) {
        self.selection = SelectionState::Selected(element.clone());

        // Update the renderer's selection state for glow effect.
        self.renderer.selected_id = cadmark_renderer::picking::encode_picking_id(&element);

        // Resolve geometry context via provenance.
        let context = cadmark_core::context::resolve_context(
            &element,
            &self.ledger,
            self.identification_strategy.as_ref(),
        );

        match context {
            Ok(context) => {
                log::info!(
                    "Selected {}: {}",
                    element.display_label(),
                    context.provenance.describe()
                );
                self.highlighted_line =
                    context.provenance.resolved().map(|entry| entry.source.line);
                self.overlay.open(
                    ScreenPosition {
                        x: screen_pos.0,
                        y: screen_pos.1,
                    },
                    context,
                );
            }
            Err(error) => {
                self.clear_selection();
                self.status = Some(Status::error(format!("Selection failed: {error}")));
            }
        }
    }

    // ── GPU helpers called from update() ────────────────────────────

    /// Upload a pending tessellated mesh to GPU buffers.
    fn drain_pending_mesh(&mut self) {
        let mesh = match self.pending_mesh.take() {
            Some(m) => m,
            None => return,
        };
        let Some(rs) = &self.wgpu_render_state else {
            return;
        };

        let gpu_mesh = cadmark_renderer::pipeline::upload_mesh(&rs.device, &mesh);

        let mut renderer = rs.renderer.write();
        if let Some(res) = renderer.callback_resources.get_mut::<ViewportResources>() {
            res.mesh = Some(gpu_mesh);
        }
        self.has_mesh = true;
        // A new mesh under a resting cursor must be picked afresh.
        self.last_hover_probe = None;
    }

    /// Check callback_resources for a decoded pick result from the
    /// previous frame's readback. If one exists, consume it.
    fn consume_pick_result(&mut self) {
        let Some(rs) = self.wgpu_render_state.clone() else {
            return;
        };

        // Extract the pick and hover results from callback_resources.
        let (completed, hover) = {
            let mut renderer = rs.renderer.write();
            let Some(res) = renderer.callback_resources.get_mut::<ViewportResources>() else {
                return;
            };
            self.hover_readback_pending = res.hover.in_flight.is_some();
            (res.pick_result.take(), res.hover.result.take())
        };

        if let Some(id) = hover {
            self.renderer.hover_id = id;
        }

        match completed_pick_transition(completed, &mut self.pick_in_flight) {
            PickTransition::Waiting => {}
            PickTransition::Background => {
                // A click on empty space puts the selection down.
                if self.overlay.is_active() {
                    self.clear_selection();
                }
            }
            PickTransition::Hit(element, screen_pos) => {
                self.handle_pick_result(element, screen_pos);
            }
            PickTransition::ReadbackFailed => {
                self.status = Some(Status::error("Selection failed: GPU pick readback failed"));
            }
        }
    }

    // ── Frame composition ───────────────────────────────────────────

    /// Keyboard shortcuts that act on the whole window. Shortcuts with a
    /// modifier are honoured everywhere except where a text field claims
    /// them; bare keys only when no text field has focus.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> ToolbarAction {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let typing = ctx.wants_keyboard_input();
        let idle = self.busy.is_none() && !self.version_dialog.is_open();
        let mut action = ToolbarAction::None;
        ctx.input_mut(|input| {
            if !typing
                && idle
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z))
            {
                action = ToolbarAction::Undo;
            } else if !typing
                && idle
                && input.consume_shortcut(&KeyboardShortcut::new(
                    Modifiers::COMMAND | Modifiers::SHIFT,
                    Key::Z,
                ))
            {
                action = ToolbarAction::Redo;
            } else if idle
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::O))
            {
                action = ToolbarAction::OpenProject;
            } else if idle
                && self.has_script
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S))
            {
                action = ToolbarAction::NameVersion;
            } else if input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::E)) {
                action = ToolbarAction::ToggleCode;
            } else if idle
                && self.has_script
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::NONE, Key::F5))
            {
                action = ToolbarAction::Refresh;
            } else if !typing
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::NONE, Key::F))
            {
                action = ToolbarAction::FitView;
            }
        });
        action
    }

    fn apply_toolbar_action(&mut self, ctx: &egui::Context, action: ToolbarAction) {
        match action {
            ToolbarAction::Undo => {
                if let Some(version) = self.history.undo() {
                    let hash = version.commit_hash.clone();
                    self.restore_version(hash);
                }
            }
            ToolbarAction::Redo => {
                if let Some(version) = self.history.redo() {
                    let hash = version.commit_hash.clone();
                    self.restore_version(hash);
                }
            }
            ToolbarAction::JumpToVersion(idx) => {
                if let Some(version) = self.history.jump_to(idx) {
                    let hash = version.commit_hash.clone();
                    self.restore_version(hash);
                }
            }
            ToolbarAction::NameVersion => self.version_dialog.open(),
            ToolbarAction::OpenProject => self.pick_project_folder(),
            ToolbarAction::OpenRecent(path) => self.open_project(ctx, path),
            ToolbarAction::RevealProject => {
                let dir = self.project_dir.clone();
                self.open_externally(&dir, "the project folder");
            }
            ToolbarAction::OpenScriptInEditor => {
                let path = self.script_path();
                self.open_externally(&path, SCRIPT_FILENAME);
            }
            ToolbarAction::Refresh => {
                log::info!("Manual refresh requested");
                self.request_reload();
            }
            ToolbarAction::FitView => {
                self.pending_camera_bounds = self.model.as_ref().and_then(|model| model.bounds);
            }
            ToolbarAction::ToggleCode => self.code_visible = !self.code_visible,
            ToolbarAction::Export(format) => self.export(format),
            ToolbarAction::None => {}
        }
    }

    fn show_code_panel(&mut self, ctx: &egui::Context) {
        let mut action = CodePanelAction::None;
        egui::TopBottomPanel::bottom("code_panel")
            .resizable(true)
            .default_height(240.0)
            .height_range(120.0..=600.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ctx, |ui| {
                action = self.code_panel.show(
                    ui,
                    CodeView {
                        script_filename: SCRIPT_FILENAME,
                        source: self.script_source.as_deref(),
                        highlighted_line: self.highlighted_line,
                        modified_on_disk: self.script_modified_on_disk,
                        controls_enabled: self.busy.is_none(),
                    },
                );
            });
        match action {
            CodePanelAction::Copy => {
                if let Some(source) = &self.script_source {
                    ctx.copy_text(source.clone());
                    self.status = Some(Status::info(format!("Copied {SCRIPT_FILENAME}")));
                }
            }
            CodePanelAction::OpenInEditor => {
                let path = self.script_path();
                self.open_externally(&path, SCRIPT_FILENAME);
            }
            CodePanelAction::Refresh => self.request_reload(),
            CodePanelAction::None => {}
        }
    }

    fn show_viewport(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::NONE.fill(cadmark_ui::theme::VIEWPORT);
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            let available = ui.available_size();
            let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());

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
            if response.hovered() && scroll.abs() > 0.1 {
                self.renderer.camera.zoom(scroll * 0.01);
            }

            // Left click for selection — request a pick readback.
            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
            {
                let local_pos = pos - rect.min;
                // Local coordinates feed the GPU picking texture.
                self.pending_pick = Some((local_pos.x, local_pos.y));
                // Absolute coordinates position the overlay on result.
                self.pick_in_flight = Some((pos.x, pos.y));
                log::debug!("Pick requested at ({}, {})", local_pos.x, local_pos.y);
            }

            // Escape cancels selection and overlay.
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.clear_selection();
            }

            // Hover: pick under the cursor while it rests over the model,
            // but not mid-drag, when the view is moving under it.
            let hover_local = if self.has_mesh && response.hovered() && !response.dragged() {
                response.hover_pos().map(|pos| pos - rect.min)
            } else {
                None
            };
            if hover_local.is_none() {
                self.renderer.hover_id = 0;
            }
            if self.renderer.hover_id != 0 {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }

            // ── Viewport rendering ──────────────────────────────────

            // Background rect — always drawn so the viewport has a
            // consistent fill even before any mesh is loaded.
            ui.painter()
                .rect_filled(rect, 0.0, cadmark_ui::theme::VIEWPORT);

            // Build the paint callback that drives the wgpu renderer.
            let ppp = ctx.pixels_per_point();
            let viewport_size = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
            let aspect = rect.width() / rect.height().max(1.0);

            if let Some(bounds) = self.pending_camera_bounds.take() {
                self.renderer.camera.frame_bounds(bounds, aspect);
                log::info!(
                    "Camera framed model at {:?}, distance {}",
                    self.renderer.camera.target,
                    self.renderer.camera.distance
                );
            }

            // Consume the pending pick (local coords → pixel coords).
            let to_pixels = |local: egui::Vec2| ((local.x * ppp) as u32, (local.y * ppp) as u32);
            let pick_request = self
                .pending_pick
                .take()
                .map(|(x, y)| to_pixels(egui::vec2(x, y)));
            let hover_request = hover_local.map(to_pixels).filter(|&pixel| {
                let probe = (pixel, self.renderer.camera.clone());
                if self.last_hover_probe.as_ref() == Some(&probe) {
                    return false;
                }
                self.last_hover_probe = Some(probe);
                true
            });
            if hover_request.is_some() {
                self.hover_readback_pending = true;
            }

            let callback = eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                ViewportCallback {
                    mesh_uniforms: self.renderer.mesh_uniforms(aspect),
                    simple_uniforms: self.renderer.simple_uniforms(aspect),
                    pick_request,
                    hover_request,
                    viewport_size,
                    clear_colour: viewport_clear_colour(self.renderer.target_is_srgb),
                },
            );
            ui.painter().add(callback);

            // Placeholder when no mesh is loaded yet — show the status
            // (including errors) so issues are visible without checking logs.
            if !self.has_mesh {
                self.paint_viewport_placeholder(ui, rect);
            }

            // Show the spatial comment overlay if active.
            let overlay_action = self.overlay.show(ui, rect);
            match overlay_action {
                cadmark_ui::overlay::OverlayAction::Submit { text, context } => {
                    self.send_spatial_comment(text, context);
                    self.overlay.close();
                }
                cadmark_ui::overlay::OverlayAction::Cancel => {
                    self.clear_selection();
                }
                cadmark_ui::overlay::OverlayAction::None => {}
            }
        });
    }

    /// What the empty viewport says: what is happening, what went wrong, or
    /// how to begin.
    fn paint_viewport_placeholder(&self, ui: &egui::Ui, rect: egui::Rect) {
        use cadmark_ui::theme;
        let (headline, detail, colour) = match (&self.busy, &self.status) {
            (Some(busy), _) => (busy.label().to_string(), String::new(), theme::TEXT_MUTED),
            (None, Some(status)) if status.is_error => (
                "The script did not run".to_string(),
                status.text.clone(),
                theme::ERROR,
            ),
            (None, _) if !self.has_script => (
                "No part yet".to_string(),
                if self.ai_model.is_some() {
                    "Describe what to build in the chat, and the model will appear here."
                        .to_string()
                } else {
                    format!("Write {SCRIPT_FILENAME} in the project folder and press Rebuild.")
                },
                theme::TEXT_MUTED,
            ),
            (None, Some(status)) => (status.text.clone(), String::new(), theme::TEXT_MUTED),
            (None, None) => (
                "Loading\u{2026}".to_string(),
                String::new(),
                theme::TEXT_MUTED,
            ),
        };
        let painter = ui.painter();
        let centre = rect.center();
        painter.text(
            centre - egui::vec2(0.0, 12.0),
            egui::Align2::CENTER_CENTER,
            &headline,
            egui::FontId::proportional(theme::HEADING_SIZE + 2.0),
            colour,
        );
        if !detail.is_empty() {
            let galley = painter.layout(
                detail,
                egui::FontId::proportional(theme::BODY_SIZE),
                colour.gamma_multiply(0.8),
                (rect.width() * 0.6).max(200.0),
            );
            let size = galley.size();
            painter.galley(
                egui::pos2(centre.x - size.x * 0.5, centre.y + 10.0),
                galley,
                colour,
            );
        }
    }
}

impl eframe::App for CadmarkApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll for async results from the worker.
        self.poll_results(ctx);
        self.watch_script();

        // Drain pending GPU work before building the frame.
        self.consume_pick_result();
        self.drain_pending_mesh();

        // Request continuous repaints while the worker is busy (to poll
        // results) or when a pick readback is in flight.
        if self.busy.is_some()
            || self.exports_in_flight > 0
            || self.pick_in_flight.is_some()
            || self.folder_pick_rx.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.hover_readback_pending {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.has_script {
            ctx.request_repaint_after(SCRIPT_WATCH_INTERVAL);
        }

        let shortcut_action = self.handle_shortcuts(ctx);
        self.apply_toolbar_action(ctx, shortcut_action);

        // Top toolbar.
        let mut toolbar_action = ToolbarAction::None;
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show(ctx, |ui| {
                let recent = self.recent_projects.others(&self.project_dir);
                let toolbar_state = ToolbarState {
                    project_dir: &self.project_dir,
                    script_filename: SCRIPT_FILENAME,
                    has_script: self.has_script,
                    recent_projects: &recent,
                    controls_enabled: self.busy.is_none(),
                    has_model: self.model.is_some(),
                    code_visible: self.code_visible,
                    ai_model: self.ai_model.as_deref(),
                };
                toolbar_action =
                    cadmark_ui::toolbar::show_toolbar(ui, &self.history, toolbar_state);
            });
        self.apply_toolbar_action(ctx, toolbar_action);

        // Bottom status bar — worker activity, else the last result.
        egui::TopBottomPanel::bottom("status_bar")
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ctx, |ui| {
                let selection = match &self.selection {
                    SelectionState::Selected(element) | SelectionState::Hovering(element) => {
                        Some(element.display_label())
                    }
                    SelectionState::None => None,
                };
                cadmark_ui::status::show_status_bar(
                    ui,
                    StatusView {
                        activity: self.busy.map(Busy::label),
                        status: self.status.as_ref(),
                        summary: self.model.as_ref().map(|model| &model.summary),
                        selection,
                    },
                );
            });

        // Right panel: chat pane.
        egui::SidePanel::right("chat_panel")
            .resizable(true)
            .default_width(380.0)
            .width_range(300.0..=700.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ctx, |ui| {
                self.chat.activity = self
                    .busy
                    .map(Busy::chat_activity)
                    .unwrap_or(ChatActivity::Idle);
                if let Some(message) = self.chat.show(ui, &self.conversation) {
                    self.send_chat_message(message);
                }
            });

        if self.code_visible {
            self.show_code_panel(ctx);
        }

        // Central panel: 3D viewport.
        self.show_viewport(ctx);

        match self.version_dialog.show(ctx) {
            VersionDialogAction::Save(name) => self.save_named_version(name),
            VersionDialogAction::Cancel | VersionDialogAction::None => {}
        }

        // A model arriving this frame produces its mesh after the
        // start-of-frame GPU drain. Ensure one more frame runs so it uploads.
        if self.pending_mesh.is_some() {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod pick_state_tests {
    use cadmark_core::geometry::{FaceId, TopologyElement};

    use super::{
        CompletedPick, PickAttempt, PickSubmissionDecision, PickTransition,
        completed_pick_transition, pick_submission_decision,
    };

    #[test]
    fn submitted_pick_marker_permits_readback() {
        let attempt = PickAttempt {
            pixel: (438, 466),
            submission_token: 42,
        };
        assert_eq!(
            pick_submission_decision(attempt, 42_u32.to_le_bytes()),
            PickSubmissionDecision::ReadPick
        );
    }

    #[test]
    fn stale_pick_marker_retries_the_exact_pixel() {
        let attempt = PickAttempt {
            pixel: (438, 466),
            submission_token: 42,
        };
        assert_eq!(
            pick_submission_decision(attempt, 41_u32.to_le_bytes()),
            PickSubmissionDecision::Retry((438, 466))
        );
    }

    #[test]
    fn incomplete_readback_retains_the_pick_anchor() {
        let mut in_flight = Some((120.0, 240.0));
        assert_eq!(
            completed_pick_transition(None, &mut in_flight),
            PickTransition::Waiting
        );
        assert_eq!(in_flight, Some((120.0, 240.0)));
    }

    #[test]
    fn completed_hit_consumes_and_returns_the_pick_anchor() {
        let element = TopologyElement::Face(FaceId(3));
        let mut in_flight = Some((120.0, 240.0));
        assert_eq!(
            completed_pick_transition(Some(CompletedPick::Hit(element.clone())), &mut in_flight),
            PickTransition::Hit(element, (120.0, 240.0))
        );
        assert_eq!(in_flight, None);
    }

    #[test]
    fn completed_background_clears_the_pick_anchor() {
        let mut in_flight = Some((120.0, 240.0));
        assert_eq!(
            completed_pick_transition(Some(CompletedPick::Background), &mut in_flight),
            PickTransition::Background
        );
        assert_eq!(in_flight, None);
    }
}

#[cfg(test)]
mod chat_message_tests {
    use cadmark_core::geometry::ModelSummary;

    use super::{edit_chat_message, viewport_clear_colour};

    fn summary(volume: f64, faces: usize) -> ModelSummary {
        ModelSummary {
            volume,
            bounds_min: [0.0; 3],
            bounds_max: [10.0, 10.0, 10.0],
            face_count: faces,
            edge_count: 0,
            vertex_count: 0,
        }
    }

    #[test]
    fn first_model_reports_its_measurements() {
        let message = edit_chat_message("Made a box", None, &summary(1000.0, 6));
        assert_eq!(
            message,
            "Made a box\n\nModel: 6 faces, volume 1000 mm³, 10 × 10 × 10 mm."
        );
    }

    #[test]
    fn edit_reports_the_measured_change_or_its_absence() {
        let before = summary(1000.0, 6);
        let changed = edit_chat_message("Added a hole", Some(&before), &summary(900.0, 9));
        assert!(changed.starts_with(
            "Added a hole\n\nModel change: faces 6 to 9; volume 1000 to 900 mm³ (-10.0%)"
        ));
        let same = edit_chat_message("Renamed a parameter", Some(&before), &before);
        assert_eq!(
            same,
            "Renamed a parameter\n\nModel unchanged: same volume, size and face count."
        );
    }

    #[test]
    fn viewport_clear_matches_the_theme_in_either_colour_space() {
        let linear = viewport_clear_colour(true);
        let gamma = viewport_clear_colour(false);
        // Linear values are darker than their gamma encoding for a dark grey.
        assert!(linear.r < gamma.r);
        let theme = cadmark_ui::theme::VIEWPORT;
        assert!((gamma.r - f64::from(theme.r()) / 255.0).abs() < 1e-9);
        assert!((gamma.g - f64::from(theme.g()) / 255.0).abs() < 1e-9);
        assert!((gamma.b - f64::from(theme.b()) / 255.0).abs() < 1e-9);
    }
}
