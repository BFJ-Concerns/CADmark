// The application: composes the panels each frame, turns what the user
// did into commands for the open project's worker, and applies what the
// worker reports. State that belongs to one project lives in `project`;
// GPU work lives in `viewport`; each panel's own state lives in the UI
// crate. Adding a panel is a field, a `show_*` method, and a call from
// `update`.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use cadmark_bridge::config::AiConfiguration;
use cadmark_bridge::grounding::GroundedComment;
use cadmark_core::export::ExportFormat;
use cadmark_core::geometry::{GeometryContext, ScreenPosition, SelectionState, TopologyElement};
use cadmark_core::message::{Conversation, Message, MessageId, ToolActivity};
use cadmark_renderer::camera::{Bounds3, Camera, Projection, StandardView};
use cadmark_renderer::pipeline::Renderer;
use cadmark_ui::chat::{ChatAction, ChatActivity, ChatPane, TurnStatus};
use cadmark_ui::code_panel::{CodePanel, CodePanelAction, CodeView};
use cadmark_ui::overlay::{OverlayAction, OverlayState};
use cadmark_ui::settings_dialog::{SettingsAction, SettingsDialog, SettingsForm};
use cadmark_ui::status::{Status, StatusView};
use cadmark_ui::toolbar::{self, ToolbarAction, ToolbarState};
use cadmark_ui::version_dialog::{VersionDialog, VersionDialogAction};
use cadmark_ui::view_gizmo::GizmoAction;

use crate::orchestrator::OrchestratorResult;
use crate::project::{Busy, Project, SCRIPT_FILENAME, SCRIPT_WATCH_INTERVAL};
use crate::turn::{TurnEvent, TurnInput, TurnOutcome, reference_images};
use crate::user_settings::{CREDENTIAL_ENV, SettingsStore, UserSettings};
use crate::viewport::{
    PickTransition, ViewportCallback, ViewportResources, completed_pick_transition,
    viewport_clear_colour,
};

/// The chat line for a completed turn: the AI's summary, then what
/// measurably changed so an edit that did more than asked is visible.
fn turn_chat_message(
    response_message: &str,
    before: Option<&cadmark_core::geometry::ModelSummary>,
    after: &cadmark_core::geometry::ModelSummary,
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

/// The running turn's chat bookkeeping: which message its text streams
/// into, and the tool calls made so far.
struct TurnRecord {
    /// The AI response message the streamed text grows.
    response: MessageId,
    /// The tool-call group message, created on the first call.
    tools: Option<MessageId>,
    /// The spatial comments the turn is acting on.
    comment_ids: Vec<MessageId>,
    /// The model summary before the turn, for the change report.
    summary_before: Option<cadmark_core::geometry::ModelSummary>,
}

/// Top-level application state.
pub struct CadmarkApp {
    project: Project,
    settings: UserSettings,
    settings_store: Option<SettingsStore>,
    chat: ChatPane,
    overlay: OverlayState,
    renderer: Renderer,
    selection: SelectionState,
    code_panel: CodePanel,
    code_visible: bool,
    /// Source line of the selected element, when its provenance is known.
    highlighted_line: Option<u32>,
    version_dialog: VersionDialog,
    settings_dialog: SettingsDialog,
    /// A folder picker running on its own thread reports here.
    folder_pick_rx: Option<mpsc::Receiver<Option<PathBuf>>>,
    /// Bounds to frame once the viewport aspect ratio is known.
    pending_camera_bounds: Option<Bounds3>,
    /// Local click coordinates (relative to viewport rect) for the
    /// pending pick request. Consumed in the same frame to build the
    /// paint callback.
    pending_pick: Option<(f32, f32)>,
    /// Absolute screen position of the in-flight pick, preserved
    /// across frames so the overlay can be positioned when the readback
    /// arrives.
    pick_in_flight: Option<(f32, f32)>,
    /// Whether a hover readback was pending at the start of this frame, so
    /// the frame that collects it is scheduled.
    hover_readback_pending: bool,
    /// The cursor pixel and camera the last hover pick was issued for. A
    /// frame that changes neither issues no new pick, so an idle cursor
    /// costs nothing.
    last_hover_probe: Option<((u32, u32), Camera)>,
    /// Whether a mesh has been uploaded to the GPU.
    has_mesh: bool,
    wgpu_render_state: Option<eframe::egui_wgpu::RenderState>,
    /// Last outcome — shown on the viewport and in the status bar.
    status: Option<Status>,
    turn: Option<TurnRecord>,
}

impl CadmarkApp {
    pub fn new(cc: &eframe::CreationContext<'_>, project_dir: PathBuf) -> Self {
        cadmark_ui::theme::apply(&cc.egui_ctx);

        let mut settings_store = SettingsStore::default_location();
        let mut status = None;
        let settings = match settings_store.as_ref().map(SettingsStore::load) {
            Some(Ok(settings)) => settings,
            Some(Err(error)) => {
                // An unreadable file is the user's to fix; nothing is
                // written over it until they save from the dialog.
                status = Some(Status::error(format!("Settings not loaded: {error}")));
                settings_store = None;
                UserSettings::default()
            }
            None => UserSettings::default(),
        };

        let wgpu_render_state = cc.wgpu_render_state.clone();
        if let Some(rs) = &wgpu_render_state {
            let resources = ViewportResources::new(&rs.device, rs.target_format);
            rs.renderer.write().callback_resources.insert(resources);
        }

        let project = Project::open(
            project_dir,
            ai_services(&settings, settings_store.as_ref()),
            settings.limits,
        );

        let mut app = Self {
            project,
            settings,
            settings_store,
            chat: ChatPane::new(),
            overlay: OverlayState::default(),
            renderer: Renderer::new(),
            selection: SelectionState::None,
            code_panel: CodePanel::default(),
            code_visible: false,
            highlighted_line: None,
            version_dialog: VersionDialog::default(),
            settings_dialog: SettingsDialog::default(),
            folder_pick_rx: None,
            pending_camera_bounds: None,
            pending_pick: None,
            pick_in_flight: None,
            hover_readback_pending: false,
            last_hover_probe: None,
            has_mesh: false,
            wgpu_render_state,
            status,
            turn: None,
        };
        app.renderer.target_is_srgb = app
            .wgpu_render_state
            .as_ref()
            .is_some_and(|rs| rs.target_format.is_srgb());
        app.chat.ai_available = app.project.ai_model.is_some();
        app.remember_project();
        app.apply_window_title(&cc.egui_ctx);
        app
    }

    fn remember_project(&mut self) {
        self.settings.remember_project(&self.project.dir);
        self.save_settings();
    }

    fn save_settings(&mut self) {
        if let Some(store) = &self.settings_store
            && let Err(error) = store.save(&self.settings)
        {
            log::warn!("Could not save settings: {error}");
        }
    }

    fn apply_window_title(&self, ctx: &egui::Context) {
        let name = toolbar::project_display_name(&self.project.dir);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{name} \u{2014} CADmark"
        )));
    }

    /// Switch to another project folder: a new worker, history and
    /// conversation, with the viewport cleared until its script has run.
    fn open_project(&mut self, ctx: &egui::Context, project_dir: PathBuf) {
        if self.project.busy.is_some() {
            return;
        }
        self.project.save_conversation();
        self.project = Project::open(
            project_dir,
            ai_services(&self.settings, self.settings_store.as_ref()),
            self.settings.limits,
        );
        self.chat = ChatPane::new();
        self.chat.ai_available = self.project.ai_model.is_some();
        self.status = None;
        self.turn = None;
        self.clear_loaded_model();
        self.renderer.camera = Camera::default();
        self.remember_project();
        self.apply_window_title(ctx);
    }

    /// Show the system folder picker on its own thread; the choice is
    /// collected in `poll_results`.
    fn pick_project_folder(&mut self, frame: &eframe::Frame) {
        if self.folder_pick_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let start_in = self
            .project
            .dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.project.dir.clone());
        let dialog = rfd::FileDialog::new()
            .set_parent(frame)
            .set_title("Open a CADmark project folder")
            .set_directory(start_in);
        std::thread::Builder::new()
            .name("cadmark-folder-picker".into())
            .spawn(move || {
                let choice = dialog.pick_folder();
                let _ = tx.send(choice);
            })
            .expect("failed to spawn the folder picker thread");
        self.folder_pick_rx = Some(rx);
    }

    // ── Turns ─────────────────────────────────────────────────────

    /// Send a chat message: one turn with this text and no anchors.
    fn send_chat_message(&mut self, text: String) {
        let history = self.project.conversation.clone();
        self.project.conversation.push(Message::user_chat(&text));
        self.start_turn(
            TurnInput {
                chat: Some(text),
                comments: Vec::new(),
                images: reference_images(&self.project.dir),
            },
            history,
            Vec::new(),
        );
    }

    /// Send a spatial comment: one turn anchored to the elements.
    fn send_spatial_comment(&mut self, text: String, anchors: Vec<GeometryContext>) {
        let history = self.project.conversation.clone();
        let id = self
            .project
            .conversation
            .push(Message::spatial_comment(&text, anchors.clone()));
        self.start_turn(
            TurnInput {
                chat: None,
                comments: vec![GroundedComment { text, anchors }],
                images: reference_images(&self.project.dir),
            },
            history,
            vec![id],
        );
    }

    /// `history` is the conversation before this turn's messages were
    /// recorded; the model sees it plus the turn's input, once.
    fn start_turn(&mut self, input: TurnInput, history: Conversation, comment_ids: Vec<MessageId>) {
        let summary_before = self
            .project
            .model
            .as_ref()
            .map(|model| model.summary.clone());
        let response = self.project.conversation.push(Message::ai_response(""));
        match self.project.start_turn(input, history) {
            Ok(_cancel) => {
                self.turn = Some(TurnRecord {
                    response,
                    tools: None,
                    comment_ids,
                    summary_before,
                });
            }
            Err(error) => {
                self.project.conversation.remove(response);
                self.status = Some(Status::error(error));
            }
        }
    }

    fn apply_turn_event(&mut self, event: TurnEvent) {
        let Some(turn) = &mut self.turn else { return };
        let conversation = &mut self.project.conversation;
        match event {
            TurnEvent::Phase(phase) => {
                self.project.note_turn_event(Some(phase));
            }
            TurnEvent::Text(text) => {
                conversation.append_text(turn.response, &text);
                self.project.note_turn_event(None);
            }
            TurnEvent::ToolStarted {
                call_id,
                tool,
                arguments,
            } => {
                let activity = ToolActivity {
                    call_id,
                    tool,
                    arguments,
                    output: None,
                    failed: false,
                    started: chrono::Utc::now(),
                    finished: None,
                };
                match turn.tools {
                    Some(id) => {
                        if let Some(Message {
                            kind: cadmark_core::message::MessageKind::ToolCalls(activities),
                            ..
                        }) = conversation.message_mut(id)
                        {
                            activities.push(activity);
                        }
                    }
                    None => {
                        // The tool group sits above the reply text: the
                        // reply is what the turn concluded after its work.
                        conversation.remove(turn.response);
                        turn.tools = Some(conversation.push(Message::tool_calls(vec![activity])));
                        turn.response = conversation.push(Message::ai_response(""));
                    }
                }
                self.project.note_turn_event(None);
            }
            TurnEvent::ToolFinished {
                call_id,
                output,
                failed,
            } => {
                if let Some(id) = turn.tools
                    && let Some(Message {
                        kind: cadmark_core::message::MessageKind::ToolCalls(activities),
                        ..
                    }) = conversation.message_mut(id)
                    && let Some(activity) = activities.iter_mut().find(|a| a.call_id == call_id)
                {
                    activity.output = Some(output);
                    activity.failed = failed;
                    activity.finished = Some(chrono::Utc::now());
                }
                self.project.note_turn_event(None);
            }
            TurnEvent::ModelBuilt { model, source } => {
                self.show_model(*model, source);
                self.project.note_turn_event(None);
            }
        }
    }

    fn finish_turn(&mut self, outcome: TurnOutcome) {
        self.project.busy = None;
        let Some(turn) = self.turn.take() else { return };
        let conversation = &mut self.project.conversation;
        match outcome {
            TurnOutcome::Completed {
                summary,
                model,
                source,
            } => {
                for id in turn.comment_ids {
                    conversation.mark_spatial_applied(id);
                }
                let reply = conversation
                    .message_mut(turn.response)
                    .map(|message| message.text.clone())
                    .unwrap_or_default();
                let text = turn_chat_message(
                    if reply.trim().is_empty() {
                        &summary
                    } else {
                        &reply
                    },
                    turn.summary_before.as_ref(),
                    &model.summary,
                );
                if let Some(message) = conversation.message_mut(turn.response) {
                    message.text = text;
                }
                match crate::git_ops::create_microversion(
                    &self.project.dir,
                    &summary,
                    &summary,
                    SCRIPT_FILENAME,
                ) {
                    Ok(version) => self.project.history.push(version),
                    Err(e) => {
                        log::error!("Failed to record the design step: {e}");
                        self.status = Some(Status::error(format!(
                            "The design step was not recorded: {e}"
                        )));
                    }
                }
                self.show_model(*model, source);
            }
            TurnOutcome::Answered => {
                if conversation
                    .message_mut(turn.response)
                    .is_some_and(|message| message.text.trim().is_empty())
                {
                    conversation.remove(turn.response);
                }
            }
            TurnOutcome::Failed { error } => {
                conversation.remove(turn.response);
                conversation.push(Message::error_notice(format!(
                    "The turn did not produce a working model, so the previous one was kept.\n\n{error}"
                )));
                self.restore_after_failed_turn();
            }
            TurnOutcome::Cancelled => {
                conversation.remove(turn.response);
                conversation.push(Message::notice("Turn cancelled; the model is as it was."));
                self.restore_after_failed_turn();
            }
        }
        self.project.save_conversation();
        self.chat.focus_input();
    }

    /// A turn that failed after a mid-turn execution left that model on
    /// screen; the script on disk is the original again, so rebuild it.
    fn restore_after_failed_turn(&mut self) {
        self.project.request_reload();
    }

    // ── Worker results ────────────────────────────────────────────

    fn poll_results(&mut self, ctx: &egui::Context) {
        for result in self.project.poll() {
            match result {
                OrchestratorResult::Reloaded { model, source } => {
                    self.project.busy = None;
                    self.show_model(*model, source);
                }
                OrchestratorResult::NoScript => {
                    self.project.busy = None;
                    self.clear_loaded_model();
                    self.project.script_source = None;
                    self.project.has_script = false;
                    self.project.script_modified_on_disk = false;
                    self.status = Some(Status::info(format!(
                        "No {SCRIPT_FILENAME} yet \u{2014} describe a part to get started"
                    )));
                }
                OrchestratorResult::ReloadFailed { error } => {
                    self.project.busy = None;
                    log::error!("Script execution failed: {error}");
                    self.clear_loaded_model();
                    self.project.has_script = self.project.script_path().exists();
                    self.project.record_script_state();
                    self.status = Some(Status::error(format!("Execution error: {error}")));
                    self.project
                        .conversation
                        .push(Message::error_notice(format!(
                            "{SCRIPT_FILENAME} failed to run.\n\n{error}"
                        )));
                }
                OrchestratorResult::TurnEvent(event) => self.apply_turn_event(event),
                OrchestratorResult::TurnEnded(outcome) => self.finish_turn(outcome),
                OrchestratorResult::Exported { format, result } => {
                    self.project.exports_in_flight =
                        self.project.exports_in_flight.saturating_sub(1);
                    self.status = Some(match result {
                        Ok(path) => Status::info(format!(
                            "Exported {} to {}",
                            format.label(),
                            path.display()
                        )),
                        Err(error) => {
                            Status::error(format!("{} export failed: {error}", format.label()))
                        }
                    });
                }
            }
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

    /// Put an executed model on screen: the GPU mesh, the ledger, the
    /// status line, and a fresh selection.
    fn show_model(&mut self, model: cadmark_kernel::protocol::ExecutedModel, source: String) {
        let mut status = format!("Built {SCRIPT_FILENAME}");
        let untraced = model.ledger.untraced_count();
        if untraced > 0 {
            status.push_str(&format!(
                " ({untraced} elements have no traceable source line)"
            ));
        }
        if !model.is_printable() {
            status.push_str(" \u{2014} not a closed valid solid");
        }
        self.status = Some(Status::info(status));

        // Picking IDs belong to the model they were assigned for.
        self.clear_selection();
        if let Some(rs) = &self.wgpu_render_state {
            let mut renderer = rs.renderer.write();
            if let Some(res) = renderer.callback_resources.get_mut::<ViewportResources>() {
                res.set_mesh(&rs.device, Some(&model.mesh));
                res.clear_picks();
            }
        }
        self.has_mesh = true;
        // A new mesh under a resting cursor must be picked afresh.
        self.last_hover_probe = None;
        if let Some(bounds) = self.project.install_model(model, source) {
            self.pending_camera_bounds = Some(bounds);
        }
    }

    fn clear_selection(&mut self) {
        self.selection = SelectionState::None;
        self.renderer.selected_id = 0;
        self.renderer.hover_id = 0;
        self.highlighted_line = None;
        self.overlay.close();
    }

    /// Clear the loaded model and any selection so the viewport cannot
    /// show stale geometry after a reload failure.
    fn clear_loaded_model(&mut self) {
        self.pending_camera_bounds = None;
        self.project.clear_model();
        self.has_mesh = false;
        self.pending_pick = None;
        self.pick_in_flight = None;
        self.clear_selection();
        if let Some(rs) = &self.wgpu_render_state {
            let mut renderer = rs.renderer.write();
            if let Some(res) = renderer.callback_resources.get_mut::<ViewportResources>() {
                res.set_mesh(&rs.device, None);
                res.clear_picks();
            }
        }
    }

    /// Check out a design step and rebuild the model from it.
    fn restore_version(&mut self, commit_hash: String) {
        match crate::git_ops::checkout_commit(&self.project.dir, &commit_hash) {
            Ok(()) => {
                log::info!("Restored design step {commit_hash}");
                self.project.request_reload();
            }
            Err(e) => {
                log::error!("Restoring {commit_hash} failed: {e}");
                self.status = Some(Status::error(format!(
                    "Could not restore that design step: {e}"
                )));
            }
        }
    }

    fn save_named_version(&mut self, name: String) {
        match crate::git_ops::create_snapshot(&self.project.dir, &name, SCRIPT_FILENAME) {
            Ok(version) => {
                self.project.history.push(version);
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

    fn export(&mut self, format: ExportFormat) {
        match self.project.request_export(format) {
            Ok(_path) => {
                self.status = Some(Status::info(format!(
                    "Exporting {}\u{2026}",
                    format.label()
                )));
            }
            Err(error) => self.status = Some(Status::error(error)),
        }
    }

    /// Open a path with the system's default handler, reporting failure in
    /// the status bar.
    fn open_externally(&mut self, path: &Path, what: &str) {
        if let Err(error) = open::that_detached(path) {
            self.status = Some(Status::error(format!("Could not open {what}: {error}")));
        }
    }

    /// Handle a completed pick — resolve to selection and open the spatial
    /// comment overlay, or add the element to an open comment.
    fn handle_pick_result(&mut self, element: TopologyElement, screen_pos: (f32, f32)) {
        let context = match cadmark_core::context::resolve_context(
            &element,
            &self.project.ledger,
            self.project.identification.as_ref(),
        ) {
            Ok(context) => context,
            Err(error) => {
                self.clear_selection();
                self.status = Some(Status::error(format!("Selection failed: {error}")));
                return;
            }
        };
        log::info!(
            "Selected {}: {}",
            element.display_label(),
            context.provenance.describe()
        );
        self.selection = SelectionState::Selected(element.clone());
        self.renderer.selected_id = cadmark_renderer::picking::encode_picking_id(&element);
        self.highlighted_line = context.provenance.resolved().map(|entry| entry.source.line);
        if !self.overlay.toggle_anchor(context.clone()) {
            self.overlay.open(
                ScreenPosition {
                    x: screen_pos.0,
                    y: screen_pos.1,
                },
                context,
            );
        }
    }

    /// Consume the pick and hover results the last frame's readbacks
    /// produced.
    fn consume_pick_result(&mut self) {
        let Some(rs) = self.wgpu_render_state.clone() else {
            return;
        };
        let (completed, hover, hover_pending) = {
            let mut renderer = rs.renderer.write();
            let Some(res) = renderer.callback_resources.get_mut::<ViewportResources>() else {
                return;
            };
            res.take_results()
        };
        self.hover_readback_pending = hover_pending;
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
                self.handle_pick_result(element, screen_pos)
            }
            PickTransition::ReadbackFailed => {
                self.status = Some(Status::error("Selection failed: GPU pick readback failed"));
            }
        }
    }

    // ── Settings ──────────────────────────────────────────────────

    fn open_settings(&mut self) {
        let store = self.settings_store.as_ref();
        let ai = self.settings.ai.clone().unwrap_or(AiConfiguration {
            base_url: String::new(),
            model: String::new(),
            accepts_images: false,
            allow_insecure_http: false,
        });
        self.settings_dialog.open(SettingsForm {
            base_url: ai.base_url,
            model: ai.model,
            accepts_images: ai.accepts_images,
            allow_insecure_http: ai.allow_insecure_http,
            credential: String::new(),
            has_stored_credential: store.is_some_and(SettingsStore::has_stored_credential),
            credential_from_environment: std::env::var_os(CREDENTIAL_ENV).is_some(),
            wall_clock_seconds: self.settings.limits.wall_clock.as_secs(),
            memory_megabytes: self.settings.limits.memory_bytes / (1024 * 1024),
        });
    }

    fn apply_settings(&mut self, ctx: &egui::Context, form: SettingsForm) {
        if self.project.busy.is_some() {
            self.settings_dialog
                .reject("Wait for the current turn to finish, then save".into());
            return;
        }
        if self.settings_store.is_none() {
            // Saving from the dialog is the explicit act that may replace
            // an unreadable file.
            self.settings_store = SettingsStore::default_location();
        }
        let ai = AiConfiguration {
            base_url: form.base_url.trim().to_string(),
            model: form.model.trim().to_string(),
            accepts_images: form.accepts_images,
            allow_insecure_http: form.allow_insecure_http,
        };
        let mut candidate = self.settings.clone();
        candidate.ai = (!ai.base_url.is_empty() || !ai.model.is_empty()).then_some(ai);
        candidate.limits = form.limits();

        if !form.credential.trim().is_empty() {
            match &self.settings_store {
                Some(store) => {
                    if let Err(error) = store.save_credential(&form.credential) {
                        self.settings_dialog
                            .reject(format!("The credential could not be stored: {error}"));
                        return;
                    }
                }
                None => {
                    self.settings_dialog.reject(
                        "No configuration directory is available to store the credential".into(),
                    );
                    return;
                }
            }
        }
        if let Err(error) = ai_services(&candidate, self.settings_store.as_ref()) {
            // A provider that cannot be constructed is not saved; the
            // dialog shows why.
            if candidate.ai.is_some() {
                self.settings_dialog.reject(error);
                return;
            }
        }

        self.settings = candidate;
        self.save_settings();
        self.settings_dialog.close();
        self.project.set_limits(self.settings.limits);
        // The provider is bound at project open; reopen to pick it up.
        let dir = self.project.dir.clone();
        self.open_project(ctx, dir);
        self.status = Some(Status::info("Settings saved"));
    }

    // ── Frame composition ─────────────────────────────────────────

    /// Keyboard shortcuts that act on the whole window. Shortcuts with a
    /// modifier are honoured everywhere except where a text field claims
    /// them; bare keys only when no text field has focus.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> ToolbarAction {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let typing = ctx.wants_keyboard_input();
        let idle = self.project.busy.is_none()
            && !self.version_dialog.is_open()
            && !self.settings_dialog.is_open();
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
                && self.project.has_script
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S))
            {
                action = ToolbarAction::NameVersion;
            } else if input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::E)) {
                action = ToolbarAction::ToggleCode;
            } else if input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma))
            {
                action = ToolbarAction::OpenSettings;
            } else if idle
                && self.project.has_script
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::NONE, Key::F5))
            {
                action = ToolbarAction::Refresh;
            } else if !typing
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::NONE, Key::F))
            {
                action = ToolbarAction::FitView;
            } else if !typing
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::NONE, Key::P))
            {
                action = ToolbarAction::ToggleProjection;
            }
        });
        action
    }

    fn apply_toolbar_action(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        action: ToolbarAction,
    ) {
        match action {
            ToolbarAction::Undo => {
                if let Some(version) = self.project.history.undo() {
                    let hash = version.commit_hash.clone();
                    self.restore_version(hash);
                }
            }
            ToolbarAction::Redo => {
                if let Some(version) = self.project.history.redo() {
                    let hash = version.commit_hash.clone();
                    self.restore_version(hash);
                }
            }
            ToolbarAction::JumpToVersion(idx) => {
                if let Some(version) = self.project.history.jump_to(idx) {
                    let hash = version.commit_hash.clone();
                    self.restore_version(hash);
                }
            }
            ToolbarAction::NameVersion => self.version_dialog.open(),
            ToolbarAction::OpenProject => self.pick_project_folder(frame),
            ToolbarAction::OpenRecent(path) => self.open_project(ctx, path),
            ToolbarAction::RevealProject => {
                let dir = self.project.dir.clone();
                self.open_externally(&dir, "the project folder");
            }
            ToolbarAction::OpenScriptInEditor => {
                let path = self.project.script_path();
                self.open_externally(&path, SCRIPT_FILENAME);
            }
            ToolbarAction::Refresh => self.project.request_reload(),
            ToolbarAction::FitView => {
                self.pending_camera_bounds =
                    self.project.model.as_ref().and_then(|model| model.bounds);
            }
            ToolbarAction::ToggleCode => self.code_visible = !self.code_visible,
            ToolbarAction::Export(format) => self.export(format),
            ToolbarAction::OpenSettings => self.open_settings(),
            ToolbarAction::ToggleProjection => {
                let next = match self.renderer.camera.projection() {
                    Projection::Perspective => Projection::Orthographic,
                    Projection::Orthographic => Projection::Perspective,
                };
                self.renderer.camera.set_projection(next);
            }
            ToolbarAction::StandardView(view) => {
                self.renderer.camera.look_at_standard(standard_view(view));
            }
            ToolbarAction::None => {}
        }
    }

    fn show_toolbar(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let mut action = ToolbarAction::None;
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show(ctx, |ui| {
                let recent = self.settings.other_recent_projects(&self.project.dir);
                let state = ToolbarState {
                    project_dir: &self.project.dir,
                    script_filename: SCRIPT_FILENAME,
                    has_script: self.project.has_script,
                    recent_projects: &recent,
                    controls_enabled: self.project.busy.is_none(),
                    has_model: self.project.model.is_some(),
                    code_visible: self.code_visible,
                    orthographic: self.renderer.camera.projection() == Projection::Orthographic,
                    printable: self.project.model.as_ref().map(|model| model.printable),
                    ai_model: self.project.ai_model.as_deref(),
                };
                action = toolbar::show_toolbar(ui, &self.project.history, state);
            });
        self.apply_toolbar_action(ctx, frame, action);
    }

    fn show_status_bar(&mut self, ctx: &egui::Context) {
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
                let activity = self.project.busy.as_ref().map(Busy::label);
                cadmark_ui::status::show_status_bar(
                    ui,
                    StatusView {
                        activity: activity.as_deref(),
                        status: self.status.as_ref(),
                        summary: self.project.model.as_ref().map(|model| &model.summary),
                        selection,
                    },
                );
            });
    }

    fn show_chat(&mut self, ctx: &egui::Context) {
        let mut action = ChatAction::None;
        egui::SidePanel::right("chat_panel")
            .resizable(true)
            .default_width(380.0)
            .width_range(300.0..=700.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ctx, |ui| {
                self.chat.activity = match &self.project.busy {
                    None => ChatActivity::Idle,
                    Some(Busy::Building) => ChatActivity::Building,
                    Some(Busy::Turn {
                        started,
                        last_event,
                        phase,
                        ..
                    }) => ChatActivity::Turn(TurnStatus {
                        phase: phase.clone(),
                        started: *started,
                        last_event: *last_event,
                    }),
                };
                action = self.chat.show(ui, &self.project.conversation);
            });
        match action {
            ChatAction::Send(text) => self.send_chat_message(text),
            ChatAction::Cancel => self.project.cancel_turn(),
            ChatAction::None => {}
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
                        source: self.project.script_source.as_deref(),
                        highlighted_line: self.highlighted_line,
                        modified_on_disk: self.project.script_modified_on_disk,
                        controls_enabled: self.project.busy.is_none(),
                    },
                );
            });
        match action {
            CodePanelAction::Copy => {
                if let Some(source) = &self.project.script_source {
                    ctx.copy_text(source.clone());
                    self.status = Some(Status::info(format!("Copied {SCRIPT_FILENAME}")));
                }
            }
            CodePanelAction::OpenInEditor => {
                let path = self.project.script_path();
                self.open_externally(&path, SCRIPT_FILENAME);
            }
            CodePanelAction::Refresh => self.project.request_reload(),
            CodePanelAction::None => {}
        }
    }

    fn show_viewport(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::NONE.fill(cadmark_ui::theme::VIEWPORT);
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            let available = ui.available_size();
            let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());

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
                self.pending_pick = Some((local_pos.x, local_pos.y));
                self.pick_in_flight = Some((pos.x, pos.y));
            }

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

            ui.painter()
                .rect_filled(rect, 0.0, cadmark_ui::theme::VIEWPORT);

            let ppp = ctx.pixels_per_point();
            let viewport_size = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
            let aspect = rect.width() / rect.height().max(1.0);

            if let Some(bounds) = self.pending_camera_bounds.take() {
                self.renderer.camera.frame_bounds(bounds, aspect);
            }

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

            if !self.has_mesh {
                self.paint_viewport_placeholder(ui, rect);
            }

            if self.has_mesh {
                let view = self.renderer.camera.view_matrix();
                let axes =
                    std::array::from_fn(|axis| [view[axis][0], view[axis][1], view[axis][2]]);
                match cadmark_ui::view_gizmo::show(ui, rect, axes) {
                    GizmoAction::LookFrom(direction) => self.renderer.camera.look_from(direction),
                    GizmoAction::Orbit(delta) => self.renderer.camera.orbit(delta.x, delta.y),
                    GizmoAction::None => {}
                }
            }

            match self.overlay.show(ui, rect) {
                OverlayAction::Submit { text, anchors } => {
                    self.overlay.close();
                    self.send_spatial_comment(text, anchors);
                }
                OverlayAction::Cancel => self.clear_selection(),
                OverlayAction::None => {}
            }
        });
    }

    /// What the empty viewport says: what is happening, what went wrong, or
    /// how to begin.
    fn paint_viewport_placeholder(&self, ui: &egui::Ui, rect: egui::Rect) {
        use cadmark_ui::theme;
        let (headline, detail, colour) = match (&self.project.busy, &self.status) {
            (Some(busy), _) => (busy.label(), String::new(), theme::TEXT_MUTED),
            (None, Some(status)) if status.is_error => (
                "The script did not run".to_string(),
                status.text.clone(),
                theme::ERROR,
            ),
            (None, _) if !self.project.has_script => (
                "No part yet".to_string(),
                if self.project.ai_model.is_some() {
                    "Describe what to build in the chat, and the model will appear here."
                        .to_string()
                } else {
                    format!(
                        "Write {SCRIPT_FILENAME} in the project folder and press Rebuild, or open Settings to add an AI provider."
                    )
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

fn standard_view(view: toolbar::StandardView) -> StandardView {
    match view {
        toolbar::StandardView::Front => StandardView::Front,
        toolbar::StandardView::Back => StandardView::Back,
        toolbar::StandardView::Left => StandardView::Left,
        toolbar::StandardView::Right => StandardView::Right,
        toolbar::StandardView::Top => StandardView::Top,
        toolbar::StandardView::Bottom => StandardView::Bottom,
        toolbar::StandardView::Isometric => StandardView::Isometric,
    }
}

/// Construct the AI services from the user's settings, or say why not.
fn ai_services(
    settings: &UserSettings,
    store: Option<&SettingsStore>,
) -> Result<cadmark_bridge::AiServices, String> {
    let ai = settings
        .ai
        .clone()
        .ok_or_else(|| "no AI provider is configured; open Settings to add one".to_string())?;
    let credential = store.and_then(SettingsStore::credential);
    cadmark_bridge::build_ai_services(ai, credential).map_err(|error| error.to_string())
}

impl eframe::App for CadmarkApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.poll_results(ctx);
        self.project.watch_script();
        self.consume_pick_result();

        // Keep the frame loop alive while anything is in flight.
        if self.project.busy.is_some()
            || self.project.exports_in_flight > 0
            || self.pick_in_flight.is_some()
            || self.folder_pick_rx.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.hover_readback_pending {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.project.has_script {
            ctx.request_repaint_after(SCRIPT_WATCH_INTERVAL);
        }

        let shortcut = self.handle_shortcuts(ctx);
        self.apply_toolbar_action(ctx, frame, shortcut);

        self.show_toolbar(ctx, frame);
        self.show_status_bar(ctx);
        self.show_chat(ctx);
        if self.code_visible {
            self.show_code_panel(ctx);
        }
        self.show_viewport(ctx);

        match self.version_dialog.show(ctx) {
            VersionDialogAction::Save(name) => self.save_named_version(name),
            VersionDialogAction::Cancel | VersionDialogAction::None => {}
        }
        match self.settings_dialog.show(ctx) {
            SettingsAction::Save(form) => self.apply_settings(ctx, form),
            SettingsAction::Cancel | SettingsAction::None => {}
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.project.save_conversation();
    }
}

#[cfg(test)]
mod tests {
    use cadmark_core::geometry::ModelSummary;

    use super::turn_chat_message;

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
        let message = turn_chat_message("Made a box", None, &summary(1000.0, 6));
        assert_eq!(
            message,
            "Made a box\n\nModel: 6 faces, volume 1000 mm³, 10 × 10 × 10 mm."
        );
    }

    #[test]
    fn edit_reports_the_measured_change_or_its_absence() {
        let before = summary(1000.0, 6);
        let changed = turn_chat_message("Added a hole", Some(&before), &summary(900.0, 9));
        assert!(changed.starts_with(
            "Added a hole\n\nModel change: faces 6 to 9; volume 1000 to 900 mm³ (-10.0%)"
        ));
        let same = turn_chat_message("Renamed a parameter", Some(&before), &before);
        assert_eq!(
            same,
            "Renamed a parameter\n\nModel unchanged: same volume, size and face count."
        );
    }
}
