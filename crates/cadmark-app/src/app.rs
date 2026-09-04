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
use cadmark_core::geometry::{
    GeometryContext, GeometryDescriptors, MinimumDistance, ScreenPosition, SelectionState,
    TopologyElement,
};
use cadmark_core::message::{Conversation, Message, MessageId, MessageKind, ToolActivity};
use cadmark_core::pending_comment::{PendingAnchor, PendingComment, PendingComments};
use cadmark_renderer::camera::{Bounds3, Camera, Projection, StandardView};
use cadmark_renderer::pipeline::{Renderer, ViewportMarker};
use cadmark_ui::chat::{ChatAction, ChatActivity, ChatPane, TurnStatus};
use cadmark_ui::code_panel::{CodePanel, CodePanelAction, CodeView};
use cadmark_ui::overlay::{OverlayAction, OverlayState};
use cadmark_ui::parameters::{ParameterRow, ParametersAction, ParametersPanel, ParametersView};
use cadmark_ui::part_name_dialog::{PartNameAction, PartNameDialog};
use cadmark_ui::settings_dialog::{SettingsAction, SettingsDialog, SettingsForm};
use cadmark_ui::start_view::{StartAction, StartViewState, show_start_view};
use cadmark_ui::status::{Status, StatusView};
use cadmark_ui::toolbar::{self, PartOption, ToolbarAction, ToolbarState};
use cadmark_ui::version_dialog::{VersionDialog, VersionDialogAction};
use cadmark_ui::view_gizmo::GizmoAction;

use crate::launch::LaunchTarget;
use crate::orchestrator::OrchestratorResult;
use crate::parts::{self, OpenPart};
use crate::project::{Busy, Project, SCRIPT_WATCH_INTERVAL};
use crate::render_source::{RenderGpu, SceneHandle, ViewportRender};
use crate::script_parameters::{self, Parameter};
use crate::turn::{
    NoRender, RenderSource, TurnEvent, TurnInput, TurnOutcome, context_usage,
    reference_image_count, reference_images,
};
use crate::user_settings::{CREDENTIAL_ENV, SettingsStore, UserSettings};
use crate::validity::{describe_validity, export_decision, export_warning};
use crate::viewport::{
    PickTransition, ViewportCallback, ViewportResources, completed_pick_transition,
    viewport_clear_colour,
};

/// Record a tool call starting, and return the group it joined and the
/// reply that carries what the AI says next.
///
/// Consecutive calls the AI made without speaking in between share one
/// group, so a long turn reads as one collapsed entry. Speaking closes the
/// group: whatever was said goes above a new one. That ordering is what
/// makes the announced sketch-or-solid route readable as a commitment —
/// every call in a group began after the text directly above it, so a
/// route stated before the run that changes the script sits above that
/// run, whatever the turn did earlier. A reply that said nothing is
/// dropped rather than shown as a blank card.
fn record_tool_start(
    conversation: &mut Conversation,
    tools: Option<MessageId>,
    response: MessageId,
    activity: ToolActivity,
) -> (MessageId, MessageId) {
    let spoke = conversation
        .message_mut(response)
        .is_some_and(|message| !message.text.trim().is_empty());
    if let Some(group) = tools
        && !spoke
        && let Some(Message {
            kind: MessageKind::ToolCalls(activities),
            ..
        }) = conversation.message_mut(group)
    {
        activities.push(activity);
        return (group, response);
    }
    if !spoke {
        conversation.remove(response);
    }
    let group = conversation.push(Message::tool_calls(vec![activity]));
    (group, conversation.push(Message::ai_response("")))
}

/// The chat line for a completed turn: the AI's summary, then what
/// measurably changed so an edit that did more than asked is visible.
/// What a finished turn reports about the geometry it produced: a solid
/// compared with the solid before it, or a sketch described in its own
/// terms, because a profile has nothing to compare a volume against.
enum TurnGeometry<'a> {
    Solid {
        before: Option<&'a cadmark_core::geometry::ModelSummary>,
        after: &'a cadmark_core::geometry::ModelSummary,
    },
    Sketch(&'a cadmark_core::sketch::SketchProfile),
}

/// What a freshly installed model does to the camera: the plane to face,
/// and the bounds to frame. A sketch is always read flat-on to its own
/// plane and framed to its own extent — it may follow a solid that is
/// still on screen, ghosted behind it, and the view then belongs to the
/// sketch rather than to the solid it was left over from. A solid is
/// framed only when it is the first model, so a rebuild does not move a
/// view the user has set.
fn camera_change(
    sketch: Option<&cadmark_core::sketch::SketchProfile>,
    first_bounds: Option<Bounds3>,
    model_bounds: Option<Bounds3>,
) -> (Option<[f32; 3]>, Option<Bounds3>) {
    match sketch {
        Some(sketch) => (Some(sketch.plane.normal), model_bounds.or(first_bounds)),
        None => (None, first_bounds),
    }
}

fn turn_chat_message(response_message: &str, geometry: TurnGeometry<'_>) -> String {
    let mut message = response_message.to_string();
    let (before, after) = match geometry {
        TurnGeometry::Solid { before, after } => (before, after),
        TurnGeometry::Sketch(sketch) => {
            message.push_str(&format!("\n\n{}.", sketch.describe()));
            return message;
        }
    };
    match before {
        Some(before) => match after.describe_change_from(before) {
            Some(change) => message.push_str(&format!(
                "\n\nModel change: {change}. Before: {}. After: {}.",
                before.describe(),
                after.describe()
            )),
            None => message.push_str(&format!(
                "\n\nModel unchanged. Before: {}. After: {}.",
                before.describe(),
                after.describe()
            )),
        },
        None => message.push_str(&format!("\n\nModel: {}.", after.describe())),
    }
    message
}

/// The pair C21 measures: exactly the two anchors currently held for the
/// comment the user is composing. Additional anchors remain comments only.
fn measurement_pair(anchors: &[GeometryContext]) -> Option<(TopologyElement, TopologyElement)> {
    (anchors.len() == 2).then(|| (anchors[0].element.clone(), anchors[1].element.clone()))
}

/// The readout tracks the element currently highlighted by the application;
/// comment anchors only provide the special two-element distance pair.
fn measurement_readout(
    selection: &SelectionState,
    minimum_distance: Option<MinimumDistance>,
    descriptors: Option<&GeometryDescriptors>,
) -> Option<String> {
    minimum_distance.map(MinimumDistance::describe).or_else(|| {
        let SelectionState::Selected(element) = selection else {
            return None;
        };
        descriptors
            .and_then(|descriptors| cadmark_ui::status::selection_measurement(element, descriptors))
    })
}

/// The design step's one-line summary for an edited parameter.
fn parameter_step_summary(name: &str, value: f64) -> String {
    format!("Set {name} to {value}")
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
    /// Messages before the active turn, which a condensation event may replace.
    history_len: usize,
}

/// Top-level application state.
pub struct CadmarkApp {
    /// The open project folder, or `None` before one is chosen: the
    /// application starts here and loads nothing until the user picks.
    project: Option<Project>,
    settings: UserSettings,
    settings_store: Option<SettingsStore>,
    chat: ChatPane,
    /// Editable spatial comments for the current design state, not yet sent.
    pending_comments: PendingComments,
    overlay: OverlayState,
    renderer: Renderer,
    selection: SelectionState,
    /// The completed distance for the current two-anchor selection.
    minimum_distance: Option<MinimumDistance>,
    code_panel: CodePanel,
    code_visible: bool,
    parameters_panel: ParametersPanel,
    /// The open part's module-level numeric names, re-read whenever the
    /// executed script changes.
    parameters: Vec<Parameter>,
    /// Source line of the selected element, when its provenance is known.
    highlighted_line: Option<u32>,
    /// The line the hovered candidate names, shown in place of the
    /// selection's own line while the pointer rests on a candidate row.
    candidate_line: Option<u32>,
    version_dialog: VersionDialog,
    part_dialog: PartNameDialog,
    settings_dialog: SettingsDialog,
    /// A folder picker running on its own thread reports here.
    folder_pick_rx: Option<mpsc::Receiver<Option<PathBuf>>>,
    /// Why the last attempt to open a folder came to nothing, shown on
    /// the start view where there is no status bar to carry it.
    start_notice: Option<String>,
    /// A message typed before a project was open, delivered as the first
    /// turn of the project the user then chooses.
    pending_first_message: Option<String>,
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
    /// Whether anything at all is drawn — a solid, a sketch, or both.
    /// The placeholder and the view gizmo follow this; picking follows
    /// `has_mesh`, which a sketch does not set.
    has_geometry: bool,
    wgpu_render_state: Option<eframe::egui_wgpu::RenderState>,
    /// What the worker thread renders for the AI: the mesh, camera and
    /// viewport size this thread last showed.
    scene: SceneHandle,
    /// Last outcome — shown on the viewport and in the status bar.
    status: Option<Status>,
    turn: Option<TurnRecord>,
}

impl CadmarkApp {
    pub fn new(cc: &eframe::CreationContext<'_>, target: LaunchTarget) -> Self {
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

        let scene = SceneHandle::new();
        let mut app = Self {
            project: None,
            settings,
            settings_store,
            chat: ChatPane::new(),
            pending_comments: PendingComments::default(),
            overlay: OverlayState::default(),
            renderer: Renderer::new(),
            selection: SelectionState::None,
            minimum_distance: None,
            code_panel: CodePanel::default(),
            code_visible: false,
            parameters_panel: ParametersPanel::default(),
            parameters: Vec::new(),
            highlighted_line: None,
            candidate_line: None,
            version_dialog: VersionDialog::default(),
            part_dialog: PartNameDialog::default(),
            settings_dialog: SettingsDialog::default(),
            folder_pick_rx: None,
            start_notice: None,
            pending_first_message: None,
            pending_camera_bounds: None,
            pending_pick: None,
            pick_in_flight: None,
            hover_readback_pending: false,
            last_hover_probe: None,
            has_mesh: false,
            has_geometry: false,
            wgpu_render_state,
            scene,
            status,
            turn: None,
        };
        app.renderer.target_is_srgb = app
            .wgpu_render_state
            .as_ref()
            .is_some_and(|rs| rs.target_format.is_srgb());
        if let LaunchTarget::Folder(dir) = target {
            app.open_project(&cc.egui_ctx, dir);
        }
        app.apply_window_title(&cc.egui_ctx);
        app
    }

    /// The open project, for the many places that only run with one.
    fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }

    fn project_mut(&mut self) -> Option<&mut Project> {
        self.project.as_mut()
    }

    /// Whether the worker is running something that must not be raced.
    fn busy(&self) -> bool {
        self.project
            .as_ref()
            .is_some_and(|project| project.busy.is_some())
    }

    /// Whether the open part has a script on disk.
    fn has_script(&self) -> bool {
        self.project
            .as_ref()
            .is_some_and(|project| project.has_script)
    }

    fn remember_project(&mut self) {
        let Some(dir) = self.project().map(|project| project.dir.clone()) else {
            return;
        };
        self.settings.remember_project(&dir);
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
        let title = match self.project() {
            Some(project) => format!(
                "{} \u{2014} {} \u{2014} CADmark",
                project.part().display_name(),
                toolbar::project_display_name(&project.dir)
            ),
            None => "CADmark".to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    /// Switch to another project folder: a new worker, history and
    /// conversation, with the viewport cleared until its script has run.
    fn open_project(&mut self, ctx: &egui::Context, project_dir: PathBuf) {
        if self.busy() {
            return;
        }
        if !project_dir.is_dir() {
            self.start_notice = Some(format!(
                "{} is not a folder any more.",
                project_dir.display()
            ));
            self.settings.forget_project(&project_dir);
            self.save_settings();
            return;
        }
        if let Some(project) = self.project() {
            project.save_conversation();
        }
        let project = Project::open(
            project_dir,
            None,
            ai_services(&self.settings, self.settings_store.as_ref()),
            self.settings.limits,
            render_source(&self.scene, self.wgpu_render_state.as_ref()),
        );
        self.chat = ChatPane::new();
        self.chat.ai_available = project.ai_model.is_some();
        self.project = Some(project);
        self.start_notice = None;
        self.pending_comments = PendingComments::default();
        self.status = None;
        self.turn = None;
        self.clear_loaded_model();
        self.renderer.camera = Camera::default();
        self.remember_project();
        self.apply_window_title(ctx);
        // A message typed before there was a project to send it to is
        // this project's first turn.
        if let Some(text) = self.pending_first_message.take() {
            self.send_chat_message(text);
        }
    }

    /// Show the system folder picker on its own thread; the choice is
    /// collected in `poll_results`.
    fn pick_project_folder(&mut self, frame: &eframe::Frame) {
        if self.folder_pick_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let start_in = self
            .project()
            .map(|project| {
                project
                    .dir
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| project.dir.clone())
            })
            .or_else(|| self.settings.recent_projects.first().cloned())
            .or_else(std::env::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
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
        let Some(project) = self.project_mut() else {
            return;
        };
        let history = project.conversation.clone();
        project.conversation.push(Message::user_chat(&text));
        let dir = project.dir.clone();
        self.start_turn(
            TurnInput {
                chat: Some(text),
                comments: Vec::new(),
                images: reference_images(&dir),
                context_window_tokens: self.settings.context_window_tokens,
            },
            history,
            Vec::new(),
        );
    }

    /// Leave a spatial comment pending so several comments can form one turn.
    fn stage_spatial_comment(&mut self, text: String, anchors: Vec<GeometryContext>) {
        let (project, pending_comments) = (&mut self.project, &mut self.pending_comments);
        let Some(project) = project.as_mut() else {
            return;
        };
        stage_pending_comment(&mut project.conversation, pending_comments, text, anchors);
        self.chat.focus_input();
    }

    /// Send all pending spatial comments, with optional chat text, as one turn.
    fn send_pending_comments(&mut self, chat: Option<String>) {
        if !self.pending_comments.can_send() {
            self.status = Some(Status::error(
                "Re-point or complete every pending comment before sending.",
            ));
            return;
        }
        let Some(project) = self.project() else {
            return;
        };
        let history = project.conversation.clone();
        let pending = self.pending_comments.drain();
        let mut comment_ids = Vec::with_capacity(pending.len());
        let comments = grounded_comments(&pending);
        let (chat_id, dir) = {
            let project = self.project_mut().expect("project was checked above");
            for comment in &pending {
                let anchors = comment
                    .live_anchors()
                    .expect("sendable pending comments have only live anchors");
                comment_ids.push(
                    project
                        .conversation
                        .push(Message::spatial_comment(&comment.text, anchors)),
                );
            }
            let chat_id = chat
                .as_ref()
                .map(|text| project.conversation.push(Message::user_chat(text)));
            (chat_id, project.dir.clone())
        };
        let rollback_ids = comment_ids.clone();
        if !self.start_turn(
            TurnInput {
                chat,
                comments,
                images: reference_images(&dir),
                context_window_tokens: self.settings.context_window_tokens,
            },
            history,
            comment_ids,
        ) {
            let project = self.project_mut().expect("project was checked above");
            if let Some(id) = chat_id {
                project.conversation.remove(id);
            }
            for id in rollback_ids {
                project.conversation.remove(id);
            }
            self.pending_comments.restore(pending);
        }
    }

    /// `history` is the conversation before this turn's messages were
    /// recorded; the model sees it plus the turn's input, once.
    fn start_turn(
        &mut self,
        input: TurnInput,
        history: Conversation,
        comment_ids: Vec<MessageId>,
    ) -> bool {
        let history_len = history.len();
        let Some(project) = self.project.as_mut() else {
            return false;
        };
        let summary_before = project
            .model
            .as_ref()
            .and_then(|model| model.summary().cloned());
        let response = project.conversation.push(Message::ai_response(""));
        match project.start_turn(input, history) {
            Ok(_cancel) => {
                self.turn = Some(TurnRecord {
                    response,
                    tools: None,
                    comment_ids,
                    summary_before,
                    history_len,
                });
                true
            }
            Err(error) => {
                project.conversation.remove(response);
                self.status = Some(Status::error(error));
                false
            }
        }
    }

    fn apply_turn_event(&mut self, event: TurnEvent) {
        if self.turn.is_none() || self.project.is_none() {
            return;
        }
        if let TurnEvent::ModelBuilt { model, source } = event {
            self.show_model(*model, source);
            if let Some(project) = self.project.as_mut() {
                project.note_turn_event(None);
            }
            return;
        }
        let Some(turn) = &mut self.turn else { return };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let conversation = &mut project.conversation;
        match event {
            TurnEvent::ConversationCondensed { summary } => {
                conversation.condense_before(turn.history_len, summary);
                turn.history_len = 1;
            }
            TurnEvent::Phase(phase) => {
                project.note_turn_event(Some(phase));
            }
            TurnEvent::Text(text) => {
                conversation.append_text(turn.response, &text);
                project.note_turn_event(None);
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
                let (tools, response) =
                    record_tool_start(conversation, turn.tools, turn.response, activity);
                turn.tools = Some(tools);
                turn.response = response;
                project.note_turn_event(None);
            }
            TurnEvent::ToolFinished {
                call_id,
                output,
                failed,
            } => {
                if let Some(id) = turn.tools
                    && let Some(Message {
                        kind: MessageKind::ToolCalls(activities),
                        ..
                    }) = conversation.message_mut(id)
                    && let Some(activity) = activities.iter_mut().find(|a| a.call_id == call_id)
                {
                    activity.output = Some(output);
                    activity.failed = failed;
                    activity.finished = Some(chrono::Utc::now());
                }
                project.note_turn_event(None);
            }
            // Handled before the project is borrowed above.
            TurnEvent::ModelBuilt { .. } => {}
        }
    }

    fn finish_turn(&mut self, outcome: TurnOutcome) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        project.busy = None;
        let Some(turn) = self.turn.take() else { return };
        let conversation = &mut project.conversation;
        // What the outcome asks of the rest of the application, once the
        // conversation has been brought up to date.
        let mut show: Option<(Box<cadmark_kernel::protocol::ExecutedModel>, String)> = None;
        let mut rebuild = false;
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
                    match &model.form {
                        cadmark_kernel::protocol::ModelForm::Solid(solid) => TurnGeometry::Solid {
                            before: turn.summary_before.as_ref(),
                            after: &solid.summary,
                        },
                        cadmark_kernel::protocol::ModelForm::Sketch(sketch) => {
                            TurnGeometry::Sketch(sketch)
                        }
                    },
                );
                if let Some(message) = conversation.message_mut(turn.response) {
                    message.text = text;
                }
                // A failure has already gone to the status; the reply
                // stands either way, so the turn reads no further.
                let _ = self.record_design_step(&summary, &summary);
                show = Some((model, source));
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
                rebuild = true;
            }
            TurnOutcome::Cancelled => {
                conversation.remove(turn.response);
                conversation.push(Message::notice("Turn cancelled; the model is as it was."));
                rebuild = true;
            }
        }
        if let Some((model, source)) = show {
            self.show_model(*model, source);
        }
        if rebuild {
            self.restore_after_failed_turn();
        }
        if let Some(project) = self.project() {
            project.save_conversation();
        }
        self.chat.focus_input();
    }

    /// Record the open part's script as it now stands as a design step.
    /// Every change to the script goes through here — a completed turn
    /// and a parameter edit alike — so the script on disk and the newest
    /// step never differ (C15). A failure leaves the reason in the status
    /// and says so to the caller: the script has changed and the history
    /// has not, which no caller may report as success.
    fn record_design_step(&mut self, summary: &str, trigger: &str) -> Result<(), String> {
        let Some((dir, script_filename)) = self
            .project()
            .map(|project| (project.dir.clone(), project.part_file_name().to_string()))
        else {
            return Err("No project is open.".to_string());
        };
        match crate::git_ops::create_microversion(&dir, summary, trigger, &script_filename) {
            Ok(_) => {
                if let Some(project) = self.project_mut() {
                    project.reload_history();
                }
                Ok(())
            }
            Err(e) => {
                log::error!("Failed to record the design step: {e}");
                let reason = format!("The design step was not recorded: {e}");
                self.status = Some(Status::error(reason.clone()));
                Err(reason)
            }
        }
    }

    /// A turn that failed after a mid-turn execution left that model on
    /// screen; the script on disk is the original again, so rebuild it.
    fn restore_after_failed_turn(&mut self) {
        if let Some(project) = self.project_mut() {
            project.request_reload();
        }
    }

    // ── Worker results ────────────────────────────────────────────

    fn poll_results(&mut self, ctx: &egui::Context) {
        let results = match self.project_mut() {
            Some(project) => project.poll(),
            None => Vec::new(),
        };
        for result in results {
            let Some(project) = self.project.as_mut() else {
                break;
            };
            let part = project.part_file_name().to_string();
            match result {
                OrchestratorResult::Reloaded { model, source } => {
                    project.busy = None;
                    self.show_model(*model, source);
                }
                OrchestratorResult::NoScript => {
                    project.busy = None;
                    project.script_source = None;
                    project.has_script = false;
                    project.script_modified_on_disk = false;
                    self.clear_loaded_model();
                    self.status = Some(Status::info(format!(
                        "No {part} yet \u{2014} describe a part to get started"
                    )));
                }
                OrchestratorResult::ReloadFailed { error } => {
                    project.busy = None;
                    log::error!("Script execution failed: {error}");
                    project.has_script = project.script_path().exists();
                    project.record_script_state();
                    project.conversation.push(Message::error_notice(format!(
                        "{part} failed to run.\n\n{error}"
                    )));
                    self.clear_loaded_model();
                    self.status = Some(Status::error(format!("Execution error: {error}")));
                }
                OrchestratorResult::TurnEvent(event) => self.apply_turn_event(event),
                OrchestratorResult::TurnEnded(outcome) => self.finish_turn(outcome),
                OrchestratorResult::Exported { format, result } => {
                    project.exports_in_flight = project.exports_in_flight.saturating_sub(1);
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
                OrchestratorResult::MinimumDistanceMeasured {
                    first,
                    second,
                    result,
                } => {
                    project.measurements_in_flight =
                        project.measurements_in_flight.saturating_sub(1);
                    if measurement_pair(self.overlay.anchors()) == Some((first, second)) {
                        self.minimum_distance = match result {
                            Ok(measurement) => Some(measurement),
                            Err(error) => {
                                self.status =
                                    Some(Status::error(format!("Measurement failed: {error}")));
                                None
                            }
                        };
                    }
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
                    // Nothing was chosen, so a message held for the
                    // project goes back into the input rather than
                    // waiting on a choice the user declined to make.
                    if let Some(text) = self.pending_first_message.take() {
                        self.chat.input_text = text;
                        self.start_notice = None;
                        self.chat.focus_input();
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    /// Put an executed model on screen: the GPU mesh, the ledger, the
    /// status line, and a fresh selection.
    fn show_model(&mut self, model: cadmark_kernel::protocol::ExecutedModel, source: String) {
        let part = match self.project() {
            Some(project) => project.part_file_name().to_string(),
            None => return,
        };
        let mut status = format!("Built {part}");
        let untraced = model.ledger.untraced_count();
        if untraced > 0 {
            status.push_str(&format!(
                " ({untraced} elements have no traceable source line)"
            ));
        }
        status.push_str(&format!(
            " \u{2014} {}",
            match &model.form {
                cadmark_kernel::protocol::ModelForm::Solid(solid) =>
                    describe_validity(&solid.validity),
                // A sketch is not an invalid solid; it is not a solid
                // yet, and the status line says which.
                cadmark_kernel::protocol::ModelForm::Sketch(sketch) => sketch.describe(),
            }
        ));
        self.status = Some(Status::info(status));

        // Picking IDs belong to the model they were assigned for.
        self.clear_selection();
        let sketch = match &model.form {
            cadmark_kernel::protocol::ModelForm::Solid(_) => None,
            cadmark_kernel::protocol::ModelForm::Sketch(sketch) => Some(sketch.clone()),
        };
        if let Some(rs) = &self.wgpu_render_state {
            let mut renderer = rs.renderer.write();
            if let Some(res) = renderer.callback_resources.get_mut::<ViewportResources>() {
                match &sketch {
                    // A sketch-only result leaves the solid last built on
                    // the GPU so it can be ghosted behind the profile.
                    Some(sketch) => res.set_sketch(&rs.device, Some(sketch)),
                    None => {
                        res.set_mesh(&rs.device, Some(&model.mesh));
                        res.set_sketch(&rs.device, None);
                    }
                }
                res.clear_picks();
            }
        }
        // Only a solid is pickable: the ghosted mesh belongs to an
        // earlier script, and this model's ledger cannot explain it.
        self.has_mesh = sketch.is_none();
        self.has_geometry = true;
        self.renderer.ghost_solid = sketch.is_some();
        // A new mesh under a resting cursor must be picked afresh.
        self.last_hover_probe = None;
        let mesh = std::sync::Arc::new(model.mesh.clone());
        let first_bounds = self
            .project_mut()
            .and_then(|project| project.install_model(model, source));
        let model_bounds = self
            .project()
            .and_then(|project| project.model.as_ref())
            .and_then(|model| model.bounds);
        let (face_plane, frame) = camera_change(sketch.as_ref(), first_bounds, model_bounds);
        match &sketch {
            Some(sketch) => self
                .scene
                .set_sketch(Some((std::sync::Arc::new(sketch.clone()), frame))),
            None => {
                self.scene.set_sketch(None);
                self.scene.set_mesh(Some((mesh, first_bounds)));
            }
        }
        if let Some(normal) = face_plane {
            self.renderer.camera.view_plane_face_on(normal);
        }
        if let Some(bounds) = frame {
            self.pending_camera_bounds = Some(bounds);
        }
        self.read_parameters();
    }

    fn clear_selection(&mut self) {
        self.selection = SelectionState::None;
        self.minimum_distance = None;
        self.renderer.selected_id = 0;
        self.renderer.hover_id = 0;
        self.highlighted_line = None;
        self.candidate_line = None;
        self.renderer.highlight_ids.clear();
        self.overlay.close();
    }

    /// Clear the loaded model and any selection so the viewport cannot
    /// show stale geometry after a reload failure.
    fn clear_loaded_model(&mut self) {
        self.parameters.clear();
        self.pending_camera_bounds = None;
        if let Some(project) = self.project_mut() {
            project.clear_model();
        }
        self.scene.set_mesh(None);
        self.scene.set_sketch(None);
        self.has_mesh = false;
        self.has_geometry = false;
        self.renderer.ghost_solid = false;
        self.pending_pick = None;
        self.pick_in_flight = None;
        self.clear_selection();
        if let Some(rs) = &self.wgpu_render_state {
            let mut renderer = rs.renderer.write();
            if let Some(res) = renderer.callback_resources.get_mut::<ViewportResources>() {
                res.set_mesh(&rs.device, None);
                res.set_sketch(&rs.device, None);
                res.clear_picks();
            }
        }
    }

    /// Check out a design step and rebuild the model from it. A step that
    /// changed another part of the folder reopens that part, so what the
    /// user sees is what the step changed.
    fn restore_version(&mut self, commit_hash: String) {
        let Some(dir) = self.project().map(|project| project.dir.clone()) else {
            return;
        };
        let restored = {
            let Some(project) = self.project_mut() else {
                return;
            };
            crate::git_ops::checkout_history_step(&project.dir, &mut project.history, &commit_hash)
        };
        match restored {
            Ok(()) => {
                log::info!("Restored design step {commit_hash}");
                let part = crate::git_ops::part_of_commit(&dir, &commit_hash);
                let Some(project) = self.project_mut() else {
                    return;
                };
                match part {
                    Some(file_name) if file_name != project.part_file_name() => {
                        let part = if file_name == crate::parts::UNTITLED_PART {
                            OpenPart::Untitled
                        } else {
                            OpenPart::Named(file_name)
                        };
                        project.switch_part(part);
                    }
                    _ => project.request_reload(),
                }
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
        let Some(project) = self.project_mut() else {
            return;
        };
        match crate::git_ops::create_snapshot(&project.dir, &name, project.part_file_name()) {
            Ok(_) => {
                project.reload_history();
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
        let Some(project) = self.project_mut() else {
            return;
        };
        match project.request_export(format) {
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

    /// Show what the candidate under the pointer accounts for: its own
    /// source line in the code panel, and the geometry the ledger
    /// attributes to its operation lit in the viewport.
    ///
    /// Where two candidates were recorded against the same elements their
    /// footprints coincide — the ledger drew no distinction there and this
    /// invents none; the code panel's line is what tells them apart.
    ///
    /// The whole ledger footprint is sent, vertices included, even though
    /// the renderer draws only faces and edges: vertices are not a drawable
    /// element class anywhere in CADmark yet — the tessellation carries no
    /// vertex positions and nothing can pick one — so their picking IDs
    /// simply match nothing this frame. Filtering them here would make the
    /// highlight set disagree with the ledger, and the set would then have
    /// to be widened again the moment a point pass exists.
    fn apply_candidate_hover(&mut self, ctx: &egui::Context) {
        let hovered = self.overlay.hovered_candidate().cloned();
        let (line, footprint) = match (&hovered, self.project.as_ref()) {
            (Some(candidate), Some(project)) => (
                Some(candidate.source.line),
                candidate_highlight_ids(&project.ledger, candidate.operation_id),
            ),
            _ => (None, Vec::new()),
        };
        if self.candidate_line != line || self.renderer.highlight_ids != footprint {
            self.candidate_line = line;
            self.renderer.highlight_ids = footprint;
            ctx.request_repaint();
        }
    }

    /// Handle a completed pick — resolve to selection and open the spatial
    /// comment overlay, or add the element to an open comment.
    fn handle_pick_result(&mut self, element: TopologyElement, screen_pos: (f32, f32)) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let context = match cadmark_core::context::resolve_context(
            &element,
            &project.ledger,
            project.identification.as_ref(),
        ) {
            Ok(context) => cadmark_core::context::with_source_context(
                context,
                project.script_source.as_deref(),
            ),
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
        self.minimum_distance = None;
        if let Some((first, second)) = measurement_pair(self.overlay.anchors())
            && let Some(project) = self.project_mut()
            && let Err(error) = project.request_minimum_distance(first, second)
        {
            self.status = Some(Status::error(format!("Measurement failed: {error}")));
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
            context_window_tokens: self.settings.context_window_tokens,
        });
    }

    fn apply_settings(&mut self, ctx: &egui::Context, form: SettingsForm) {
        if self.busy() {
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
        candidate.context_window_tokens = form.context_window_tokens.max(1_024);

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
        let limits = self.settings.limits;
        if let Some(project) = self.project_mut() {
            project.set_limits(limits);
        }
        // The provider is bound at project open; reopen to pick it up.
        if let Some(dir) = self.project().map(|project| project.dir.clone()) {
            self.open_project(ctx, dir);
        }
        self.status = Some(Status::info("Settings saved"));
    }

    // ── Frame composition ─────────────────────────────────────────

    /// Keyboard shortcuts that act on the whole window. Shortcuts with a
    /// modifier are honoured everywhere except where a text field claims
    /// them; bare keys only when no text field has focus.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> ToolbarAction {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let typing = ctx.wants_keyboard_input();
        let idle =
            !self.busy() && !self.version_dialog.is_open() && !self.settings_dialog.is_open();
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
                && self.has_script()
                && input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S))
            {
                action = ToolbarAction::NameVersion;
            } else if input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::E)) {
                action = ToolbarAction::ToggleCode;
            } else if input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma))
            {
                action = ToolbarAction::OpenSettings;
            } else if idle
                && self.has_script()
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
                if let Some(version) = self.project_mut().and_then(|p| p.history.undo()) {
                    let hash = version.commit_hash.clone();
                    self.restore_version(hash);
                }
            }
            ToolbarAction::Redo => {
                if let Some(version) = self.project_mut().and_then(|p| p.history.redo()) {
                    let hash = version.commit_hash.clone();
                    self.restore_version(hash);
                }
            }
            ToolbarAction::JumpToVersion(idx) => {
                if let Some(version) = self.project_mut().and_then(|p| p.history.jump_to(idx)) {
                    let hash = version.commit_hash.clone();
                    self.restore_version(hash);
                }
            }
            // The same key names the part the first time and a version
            // every time after: the meaning changes once, when the part
            // gets a name of its own.
            ToolbarAction::NameVersion => {
                match self.project().map(|p| parts::save_target(p.part())) {
                    Some(parts::SaveTarget::PartName) => self.part_dialog.open(),
                    Some(parts::SaveTarget::VersionName) => self.version_dialog.open(),
                    None => {}
                }
            }
            ToolbarAction::NewPart => {
                if let Some(project) = self.project_mut() {
                    match project.begin_untitled_part() {
                        Ok(()) => {
                            self.status =
                                Some(Status::info("New part \u{2014} save it to give it a name"));
                            self.clear_loaded_model();
                        }
                        Err(error) => self.status = Some(Status::error(error)),
                    }
                }
            }
            ToolbarAction::OpenPart(file_name) => {
                let part = if file_name == parts::UNTITLED_PART {
                    OpenPart::Untitled
                } else {
                    OpenPart::Named(file_name)
                };
                if let Some(project) = self.project_mut() {
                    project.switch_part(part);
                }
                self.clear_loaded_model();
                self.apply_window_title(ctx);
            }
            ToolbarAction::OpenProject => self.pick_project_folder(frame),
            ToolbarAction::NewConversation => {
                let Some(project) = self.project_mut() else {
                    return;
                };
                match project.start_fresh_conversation() {
                    Ok(()) => {
                        self.turn = None;
                        self.chat.focus_input();
                        self.status = Some(Status::info("Started a new conversation"));
                    }
                    Err(error) => self.status = Some(Status::error(error)),
                }
            }
            ToolbarAction::OpenRecent(path) => self.open_project(ctx, path),
            ToolbarAction::RevealProject => {
                if let Some(dir) = self.project().map(|project| project.dir.clone()) {
                    self.open_externally(&dir, "the project folder");
                }
            }
            ToolbarAction::OpenScriptInEditor => {
                let Some((path, what)) = self
                    .project()
                    .map(|project| (project.script_path(), project.part_file_name().to_string()))
                else {
                    return;
                };
                self.open_externally(&path, &what);
            }
            ToolbarAction::Refresh => {
                if let Some(project) = self.project_mut() {
                    project.request_reload();
                }
            }
            ToolbarAction::FitView => {
                let model = self.project().and_then(|project| project.model.as_ref());
                // Fitting a sketch returns to the flat view of its own
                // plane, which is how it was first shown.
                let plane_normal = model
                    .and_then(|model| model.sketch())
                    .map(|sketch| sketch.plane.normal);
                self.pending_camera_bounds = model.and_then(|model| model.bounds);
                if let Some(normal) = plane_normal {
                    self.renderer.camera.view_plane_face_on(normal);
                }
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
                let Some(project) = self.project.as_ref() else {
                    return;
                };
                let recent = self.settings.other_recent_projects(&project.dir);
                let export_warning = project
                    .model
                    .as_ref()
                    .and_then(|model| export_warning(&export_decision(model.validity())));
                let part_options: Vec<PartOption> = project
                    .parts()
                    .iter()
                    .map(|file_name| PartOption {
                        file_name: file_name.clone(),
                        display: parts::part_display_name(file_name),
                    })
                    .collect();
                let part_name = project.part().display_name();
                let state = ToolbarState {
                    project_dir: &project.dir,
                    script_filename: project.part_file_name(),
                    part_name: &part_name,
                    parts: &part_options,
                    has_script: project.has_script,
                    recent_projects: &recent,
                    controls_enabled: project.busy.is_none(),
                    has_model: project.model.is_some(),
                    code_visible: self.code_visible,
                    orthographic: self.renderer.camera.projection() == Projection::Orthographic,
                    export_warning: export_warning.as_deref(),
                    ai_model: project.ai_model.as_deref(),
                };
                action = toolbar::show_toolbar(ui, &project.history, state);
            });
        self.apply_toolbar_action(ctx, frame, action);
    }

    /// Give the open untitled part the name the prompt collected, or keep
    /// the prompt open saying why the name was refused.
    fn name_open_part(&mut self, ctx: &egui::Context, typed: String) {
        let Some(project) = self.project_mut() else {
            return;
        };
        match project.name_untitled_part(&typed) {
            Ok(file_name) => {
                self.status = Some(Status::info(format!(
                    "Saved as {}",
                    parts::part_display_name(&file_name)
                )));
                self.apply_window_title(ctx);
            }
            Err(reason) => self.part_dialog.reject(reason),
        }
    }

    /// The window before a project is chosen: nothing is loaded, and the
    /// only things on screen are the ways to choose one.
    fn show_start_view(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let mut action = StartAction::None;
        // Chat is the primary channel and stays reachable with no project
        // open (C39): what is typed here is held and sent as the first
        // turn of whichever project the user then chooses.
        let mut chat_action = ChatAction::None;
        self.chat.activity = ChatActivity::Idle;
        self.chat.ai_available = self.settings.ai.is_some();
        let waiting = Conversation::new();
        egui::SidePanel::right("chat_panel")
            .resizable(true)
            .default_width(380.0)
            .width_range(300.0..=700.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ctx, |ui| {
                let usage = context_usage(&waiting, 0, self.settings.context_window_tokens);
                chat_action = self
                    .chat
                    .show(ui, &waiting, usage, &mut self.pending_comments);
            });
        if let ChatAction::Send(text) = chat_action {
            self.pending_first_message = Some(text);
            self.start_notice = Some(
                "Choose a project folder \u{2014} your message is sent as soon as it opens."
                    .to_string(),
            );
            self.pick_project_folder(frame);
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            action = show_start_view(
                ui,
                StartViewState {
                    recent_projects: &self.settings.recent_projects,
                    notice: self.start_notice.as_deref(),
                    controls_enabled: self.folder_pick_rx.is_none(),
                },
            );
        });
        match action {
            // Creating a project and opening one are the same choice of
            // folder; a folder with no parts opens at its first one.
            StartAction::OpenFolder | StartAction::CreateProject => {
                self.pick_project_folder(frame);
            }
            StartAction::OpenRecent(path) => self.open_project(ctx, path),
            StartAction::OpenSettings => self.open_settings(),
            StartAction::None => {}
        }
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
                let activity = self
                    .project()
                    .and_then(|project| project.busy.as_ref())
                    .map(Busy::label);
                let measurement = measurement_readout(
                    &self.selection,
                    self.minimum_distance,
                    self.project()
                        .and_then(|project| project.model.as_ref())
                        .map(|model| &model.descriptors),
                );
                cadmark_ui::status::show_status_bar(
                    ui,
                    StatusView {
                        activity: activity.as_deref(),
                        status: self.status.as_ref(),
                        summary: self
                            .project()
                            .and_then(|project| project.model.as_ref())
                            .and_then(|model| model.summary()),
                        selection,
                        measurement: measurement.as_deref(),
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
                let Some(project) = self.project.as_ref() else {
                    return;
                };
                self.chat.activity = match &project.busy {
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
                let usage = context_usage(
                    &project.conversation,
                    reference_image_count(&project.dir),
                    self.settings.context_window_tokens,
                );
                action =
                    self.chat
                        .show(ui, &project.conversation, usage, &mut self.pending_comments);
            });
        match action {
            ChatAction::Send(text) => self.send_chat_message(text),
            ChatAction::SendPending { chat } => self.send_pending_comments(chat),
            ChatAction::RemovePending(id) => {
                self.pending_comments.remove(id);
            }
            ChatAction::Cancel => {
                if let Some(project) = self.project() {
                    project.cancel_turn();
                }
            }
            ChatAction::None => {}
        }
    }

    /// Re-read the open part's parameters from the source that was
    /// executed, so the panel always describes the model on screen.
    fn read_parameters(&mut self) {
        self.parameters = match self
            .project()
            .and_then(|project| project.script_source.as_deref())
        {
            Some(source) => script_parameters::extract(source),
            None => Vec::new(),
        };
    }

    fn show_parameters(&mut self, ctx: &egui::Context) {
        let rows: Vec<ParameterRow<'_>> = self
            .parameters
            .iter()
            .map(|parameter| ParameterRow {
                name: &parameter.name,
                value: parameter.value(),
                expression: parameter.expression().unwrap_or_default(),
                line: parameter.line,
            })
            .collect();
        let mut action = ParametersAction::None;
        egui::SidePanel::left("parameters_panel")
            .resizable(true)
            .default_width(240.0)
            .width_range(180.0..=420.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ctx, |ui| {
                let Some(project) = self.project.as_ref() else {
                    return;
                };
                action = self.parameters_panel.show(
                    ui,
                    ParametersView {
                        script_filename: project.part_file_name(),
                        parameters: &rows,
                        has_script: project.has_script,
                        controls_enabled: project.busy.is_none(),
                    },
                );
            });
        if let ParametersAction::Commit { name, value } = action {
            self.apply_parameter_edit(&name, value);
        }
    }

    /// A parameter edit: rewrite that one value in the open part's
    /// script, record the design step, and rebuild. No AI turn is
    /// involved (C16), and the step is recorded as the file is written so
    /// the newest step is always the script on disk.
    fn apply_parameter_edit(&mut self, name: &str, value: f64) {
        let Some((path, part)) = self
            .project()
            .map(|project| (project.script_path(), project.part_file_name().to_string()))
        else {
            return;
        };
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                self.status = Some(Status::error(format!("Could not read {part}: {error}")));
                return;
            }
        };
        let rewritten = match script_parameters::rewrite(&source, name, value) {
            Ok(rewritten) => rewritten,
            Err(error) => {
                self.status = Some(Status::error(error.to_string()));
                return;
            }
        };
        if rewritten == source {
            return;
        }
        if let Err(error) = std::fs::write(&path, &rewritten) {
            self.status = Some(Status::error(format!("Could not write {part}: {error}")));
            return;
        }
        let summary = parameter_step_summary(name, value);
        let recorded = self.record_design_step(&summary, &summary);
        if let Some(project) = self.project_mut() {
            project.record_script_state();
        }
        match recorded {
            Ok(()) => self.status = Some(Status::info(summary)),
            // The rebuild that follows will post its own status over
            // this one, so a divergence between the script and the
            // newest design step (C15) also goes to the conversation,
            // where the user still has it afterwards.
            Err(reason) => {
                if let Some(project) = self.project_mut() {
                    project.conversation.push(Message::error_notice(format!(
                        "{reason}\n\n{summary} in {part}, so the model will rebuild, but the history has no step for it — undo goes back past this change rather than to it."
                    )));
                    project.save_conversation();
                }
            }
        }
        if let Some(project) = self.project_mut() {
            project.request_reload();
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
                let Some(project) = self.project.as_ref() else {
                    return;
                };
                action = self.code_panel.show(
                    ui,
                    CodeView {
                        script_filename: project.part_file_name(),
                        source: project.script_source.as_deref(),
                        highlighted_line: self.candidate_line.or(self.highlighted_line),
                        modified_on_disk: project.script_modified_on_disk,
                        controls_enabled: project.busy.is_none(),
                    },
                );
            });
        match action {
            CodePanelAction::Copy => {
                let copied = self.project().and_then(|project| {
                    project
                        .script_source
                        .clone()
                        .map(|source| (source, project.part_file_name().to_string()))
                });
                if let Some((source, part)) = copied {
                    ctx.copy_text(source);
                    self.status = Some(Status::info(format!("Copied {part}")));
                }
            }
            CodePanelAction::OpenInEditor => {
                let Some((path, what)) = self
                    .project()
                    .map(|project| (project.script_path(), project.part_file_name().to_string()))
                else {
                    return;
                };
                self.open_externally(&path, &what);
            }
            CodePanelAction::Refresh => {
                if let Some(project) = self.project_mut() {
                    project.request_reload();
                }
            }
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
            // What the AI is shown when it asks for a render: this
            // thread's camera and viewport size, never its GPU target.
            self.scene
                .set_view(self.renderer.camera.clone(), viewport_size);

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

            self.refresh_pending_markers();
            let callback = eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                ViewportCallback {
                    mesh_uniforms: self.renderer.mesh_uniforms(aspect),
                    highlight_ids: self.renderer.highlight_ids.clone(),
                    simple_uniforms: self.renderer.simple_uniforms(aspect),
                    markers: self.renderer.markers.clone(),
                    pick_request,
                    hover_request,
                    viewport_size,
                    clear_colour: viewport_clear_colour(self.renderer.target_is_srgb),
                },
            );
            ui.painter().add(callback);

            if !self.has_geometry {
                self.paint_viewport_placeholder(ui, rect);
            }

            if self.has_geometry {
                let view = self.renderer.camera.view_matrix();
                let axes =
                    std::array::from_fn(|axis| [view[axis][0], view[axis][1], view[axis][2]]);
                match cadmark_ui::view_gizmo::show(ui, rect, axes) {
                    GizmoAction::LookFrom(direction) => self.renderer.camera.look_from(direction),
                    GizmoAction::Orbit(delta) => self.renderer.camera.orbit(delta.x, delta.y),
                    GizmoAction::None => {}
                }
            }

            let action = self.overlay.show(ui, rect);
            self.apply_candidate_hover(ui.ctx());
            match action {
                OverlayAction::Submit { text, anchors } => {
                    self.overlay.close();
                    self.stage_spatial_comment(text, anchors);
                }
                OverlayAction::Cancel => self.clear_selection(),
                OverlayAction::None => {}
            }
        });
    }

    /// Translate application-owned pending anchors into generic renderer
    /// markers. The renderer receives no conversation or card state.
    fn refresh_pending_markers(&mut self) {
        self.renderer.markers = pending_markers(&self.pending_comments);
    }

    /// What the empty viewport says: what is happening, what went wrong, or
    /// how to begin.
    fn paint_viewport_placeholder(&self, ui: &egui::Ui, rect: egui::Rect) {
        use cadmark_ui::theme;
        let Some(project) = self.project() else {
            return;
        };
        let (headline, detail, colour) = match (&project.busy, &self.status) {
            (Some(busy), _) => (busy.label(), String::new(), theme::TEXT_MUTED),
            (None, Some(status)) if status.is_error => (
                "The script did not run".to_string(),
                status.text.clone(),
                theme::ERROR,
            ),
            (None, _) if !project.has_script => (
                "No part yet".to_string(),
                if project.ai_model.is_some() {
                    "Describe what to build in the chat, and the model will appear here."
                        .to_string()
                } else {
                    format!(
                        "Write {} in the project folder and press Rebuild, or open Settings to add an AI provider.",
                        project.part_file_name()
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

/// Convert sendable pending cards into the comments the turn runner consumes.
fn grounded_comments(pending: &[PendingComment]) -> Vec<GroundedComment> {
    pending
        .iter()
        .map(|comment| GroundedComment {
            text: comment.text.clone(),
            anchors: comment
                .live_anchors()
                .expect("sendable pending comments have only live anchors"),
        })
        .collect()
}

/// Project the application-owned card pairing into renderer-neutral markers.
fn pending_markers(pending: &PendingComments) -> Vec<ViewportMarker> {
    pending
        .comments()
        .iter()
        .flat_map(|comment| {
            comment.anchors.iter().filter_map(|anchor| match anchor {
                PendingAnchor::Live(context) => Some(ViewportMarker {
                    element_id: cadmark_renderer::picking::encode_picking_id(&context.element),
                    colour: comment.marker_colour(),
                }),
                PendingAnchor::Lost { .. } => None,
            })
        })
        .collect()
}

/// Stage a spatial comment without changing persisted conversation history.
/// The batch enters history only when it is dispatched as a turn.
fn stage_pending_comment(
    conversation: &mut Conversation,
    pending: &mut PendingComments,
    text: String,
    anchors: Vec<GeometryContext>,
) {
    let message_count = conversation.len();
    pending.add(text, anchors);
    debug_assert_eq!(conversation.len(), message_count);
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
/// The picking IDs of every element the ledger attributes to one
/// candidate's operation — what the viewport lights up while the pointer
/// rests on that candidate's row.
///
/// The footprint is taken whole. Vertices are carried even though no pass
/// draws them, so the highlight set says what the ledger says rather than
/// what the renderer currently happens to consume.
fn candidate_highlight_ids(
    ledger: &cadmark_core::ledger::ProvenanceLedger,
    operation_id: u64,
) -> Vec<u32> {
    cadmark_core::candidates::candidate_footprint(ledger, operation_id)
        .iter()
        .map(cadmark_renderer::picking::encode_picking_id)
        .collect()
}

/// What answers the AI's render tool for one project: the viewport's
/// offscreen renderer where there is a GPU to render with, and the
/// stand-in that says so honestly where there is not.
fn render_source(
    scene: &SceneHandle,
    render_state: Option<&eframe::egui_wgpu::RenderState>,
) -> Box<dyn RenderSource> {
    match render_state {
        Some(state) => Box::new(ViewportRender::new(
            scene.clone(),
            RenderGpu {
                device: state.device.clone(),
                queue: state.queue.clone(),
            },
        )),
        None => Box::new(NoRender),
    }
}

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
        if let Some(project) = self.project_mut() {
            project.watch_script();
        }
        self.consume_pick_result();

        // Keep the frame loop alive while anything is in flight.
        let exporting = self
            .project()
            .is_some_and(|project| project.exports_in_flight > 0);
        let measuring = self
            .project()
            .is_some_and(|project| project.measurements_in_flight > 0);
        if self.busy()
            || exporting
            || measuring
            || self.pick_in_flight.is_some()
            || self.folder_pick_rx.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.hover_readback_pending {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.has_script() {
            ctx.request_repaint_after(SCRIPT_WATCH_INTERVAL);
        }

        // With no project there is nothing to act on and nothing to type
        // into: the start view owns the window, and no panel is drawn
        // present-but-dead. Settings stay reachable from it.
        if self.project.is_none() {
            self.show_start_view(ctx, frame);
            match self.settings_dialog.show(ctx) {
                SettingsAction::Save(form) => self.apply_settings(ctx, form),
                SettingsAction::Cancel | SettingsAction::None => {}
            }
            return;
        }

        let shortcut = self.handle_shortcuts(ctx);
        self.apply_toolbar_action(ctx, frame, shortcut);

        self.show_toolbar(ctx, frame);
        self.show_status_bar(ctx);
        self.show_chat(ctx);
        self.show_parameters(ctx);
        if self.code_visible {
            self.show_code_panel(ctx);
        }
        self.show_viewport(ctx);

        match self.version_dialog.show(ctx) {
            VersionDialogAction::Save(name) => self.save_named_version(name),
            VersionDialogAction::Cancel | VersionDialogAction::None => {}
        }
        match self.part_dialog.show(ctx) {
            PartNameAction::Save(name) => self.name_open_part(ctx, name),
            PartNameAction::Cancel | PartNameAction::None => {}
        }
        match self.settings_dialog.show(ctx) {
            SettingsAction::Save(form) => self.apply_settings(ctx, form),
            SettingsAction::Cancel | SettingsAction::None => {}
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(project) = self.project() {
            project.save_conversation();
        }
    }
}

#[cfg(test)]
mod tests {
    use cadmark_core::pending_comment::PendingComments;

    use std::io::{ErrorKind, Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    use cadmark_bridge::backend::{ModelItem, ModelRequest, TurnModel};
    use cadmark_core::geometry::{
        EdgeDescriptor, EdgeId, FaceId, GeometryContext, GeometryDescriptors, ModelSummary,
        SelectionState, TopologyElement,
    };
    use cadmark_core::ledger::LedgerValue;
    use cadmark_core::limits::ExecutionLimits;
    use cadmark_core::message::{Conversation, Message, MessageKind, ToolActivity};

    use super::{
        Bounds3, CadmarkApp, ChatPane, CodePanel, NoRender, OverlayState, ParametersPanel,
        PartNameDialog, Project, Renderer, SceneHandle, SettingsDialog, SettingsStore,
        TurnGeometry, TurnOutcome, TurnRecord, UserSettings, VersionDialog, ai_services,
        camera_change, candidate_highlight_ids, measurement_pair, measurement_readout,
        record_tool_start, turn_chat_message,
    };

    #[derive(Debug)]
    struct RecordedProviderRequest {
        path: String,
        authenticated: bool,
        body: serde_json::Value,
    }

    fn recording_provider(
        status: u16,
        body: serde_json::Value,
    ) -> (
        String,
        thread::JoinHandle<Result<RecordedProviderRequest, String>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Err(
                                "recording provider did not receive a request within two seconds"
                                    .into(),
                            );
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => {
                        return Err(format!(
                            "recording provider could not accept a request: {error}"
                        ));
                    }
                }
            };
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut chunk = [0_u8; 4096];
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
                if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                })
                .unwrap();
            while bytes.len() < header_end + content_length {
                let mut chunk = [0_u8; 4096];
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
            }
            let recorded = RecordedProviderRequest {
                path: headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_string(),
                authenticated: headers.lines().any(|line| {
                    line.eq_ignore_ascii_case("authorization: bearer test-only-stored-token")
                }),
                body: serde_json::from_slice(&bytes[header_end..header_end + content_length])
                    .unwrap(),
            };
            let body = body.to_string();
            let wire = format!(
                "HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(wire.as_bytes()).unwrap();
            Ok(recorded)
        });
        (format!("http://{address}/v1"), handle)
    }

    fn settings_for(base_url: String, model: &str) -> UserSettings {
        UserSettings {
            ai: Some(cadmark_bridge::config::AiConfiguration {
                base_url,
                model: model.to_string(),
                accepts_images: false,
                allow_insecure_http: true,
            }),
            ..UserSettings::default()
        }
    }

    async fn provider_error_from_settings(
        settings: &UserSettings,
        store: &SettingsStore,
    ) -> cadmark_bridge::backend::BackendError {
        let services = ai_services(settings, Some(store)).unwrap();
        let mut sink = |_| {};
        services
            .model
            .respond(
                ModelRequest {
                    instructions: "test instructions".to_string(),
                    items: vec![ModelItem::User {
                        text: "test request".to_string(),
                        images: vec![],
                    }],
                    tools: vec![],
                },
                cadmark_core::cancellation::CancelFlag::new(),
                &mut sink,
            )
            .await
            .unwrap_err()
    }

    fn app_with_pending_response(project_dir: std::path::PathBuf) -> CadmarkApp {
        let mut project = open_test_project(project_dir, None);
        let response = project.conversation.push(Message::ai_response(""));
        let mut app = app_around(project);
        app.turn = Some(TurnRecord {
            response,
            tools: None,
            comment_ids: vec![],
            summary_before: None,
            history_len: 1,
        });
        app
    }

    fn open_test_project(project_dir: std::path::PathBuf, part: Option<&str>) -> Project {
        Project::open(
            project_dir,
            part,
            Err("test provider is injected directly".to_string()),
            ExecutionLimits::default(),
            Box::new(NoRender),
        )
    }

    /// An application around an open project and nothing in flight.
    fn app_around(project: Project) -> CadmarkApp {
        CadmarkApp {
            project: Some(project),
            settings: UserSettings::default(),
            settings_store: None,
            chat: ChatPane::new(),
            pending_comments: PendingComments::default(),
            overlay: OverlayState::default(),
            renderer: Renderer::default(),
            selection: SelectionState::None,
            minimum_distance: None,
            code_panel: CodePanel::default(),
            code_visible: false,
            parameters_panel: ParametersPanel::default(),
            parameters: Vec::new(),
            highlighted_line: None,
            candidate_line: None,
            version_dialog: VersionDialog::default(),
            part_dialog: PartNameDialog::default(),
            settings_dialog: SettingsDialog::default(),
            folder_pick_rx: None,
            start_notice: None,
            pending_first_message: None,
            pending_camera_bounds: None,
            pending_pick: None,
            pick_in_flight: None,
            hover_readback_pending: false,
            last_hover_probe: None,
            has_mesh: false,
            has_geometry: false,
            wgpu_render_state: None,
            scene: SceneHandle::new(),
            status: None,
            turn: None,
        }
    }

    /// The newest commit's full message in a project folder.
    fn newest_commit_message(dir: &std::path::Path) -> String {
        let out = std::process::Command::new("git")
            .args(["log", "-1", "--format=%B"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    /// A folder holding two parts, the second of which is opened.
    fn two_part_project() -> (tempfile::TempDir, CadmarkApp) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("part.py"), "width = 10\n").unwrap();
        std::fs::write(dir.path().join("bracket.py"), "width = 80\ndepth = 40\n").unwrap();
        let project = open_test_project(dir.path().to_path_buf(), Some("bracket.py"));
        let app = app_around(project);
        (dir, app)
    }

    #[test]
    fn a_parameter_edit_rewrites_the_open_part_and_records_a_step_naming_it() {
        let (dir, mut app) = two_part_project();
        assert_eq!(app.project().unwrap().part_file_name(), "bracket.py");

        app.apply_parameter_edit("width", 90.0);

        // The open part's script carries the new value; the folder's
        // other part, which binds the same name, is untouched.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("bracket.py")).unwrap(),
            "width = 90\ndepth = 40\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("part.py")).unwrap(),
            "width = 10\n"
        );

        // The change is a design step, recorded through the same path a
        // completed turn uses, against the part it changed.
        let message = newest_commit_message(dir.path());
        assert!(message.contains("Set width to 90"), "{message}");
        assert!(message.contains("part: bracket.py"), "{message}");

        // And no AI turn was involved.
        assert!(app.turn.is_none());
    }

    #[test]
    fn a_completed_turn_records_its_design_step_against_the_open_part() {
        let (dir, mut app) = two_part_project();
        let response = app
            .project_mut()
            .unwrap()
            .conversation
            .push(Message::ai_response("Widened the bracket"));
        app.turn = Some(TurnRecord {
            response,
            tools: None,
            comment_ids: vec![],
            summary_before: None,
            history_len: 1,
        });
        std::fs::write(dir.path().join("bracket.py"), "width = 120\ndepth = 40\n").unwrap();

        app.finish_turn(TurnOutcome::Completed {
            summary: "Widen the bracket".to_string(),
            model: Box::new(solid_model()),
            source: "width = 120\ndepth = 40\n".to_string(),
        });

        let message = newest_commit_message(dir.path());
        assert!(message.contains("Widen the bracket"), "{message}");
        assert!(message.contains("part: bracket.py"), "{message}");
        // The step is in the history the undo lane reads, not only in
        // git: recording reloads it.
        assert_eq!(app.project().unwrap().history.len(), 1);
    }

    /// The plainest successful execution: a solid with nothing to draw.
    fn solid_model() -> cadmark_kernel::protocol::ExecutedModel {
        cadmark_kernel::protocol::ExecutedModel {
            mesh: cadmark_core::mesh::TessellatedMesh::default(),
            ledger: cadmark_core::ledger::ProvenanceLedger::new(),
            descriptors: GeometryDescriptors::default(),
            form: cadmark_kernel::protocol::ModelForm::Solid(
                cadmark_kernel::protocol::SolidResult {
                    summary: summary(1000.0, 6),
                    validity: vec![],
                    file: cadmark_kernel::protocol::ModelFile(std::path::PathBuf::from(
                        "/scratch/model-1.brep",
                    )),
                },
            ),
        }
    }

    #[test]
    fn a_parameter_edit_with_no_project_open_changes_nothing() {
        let (dir, mut app) = two_part_project();
        app.project = None;

        app.read_parameters();
        assert!(app.parameters.is_empty());

        app.apply_parameter_edit("width", 90.0);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("bracket.py")).unwrap(),
            "width = 80\ndepth = 40\n"
        );
        assert!(app.status.is_none());
    }

    #[test]
    fn the_panel_reads_the_parameters_of_the_script_that_was_executed() {
        let (_dir, mut app) = two_part_project();
        app.project_mut().unwrap().script_source =
            Some("plate = 80\nmargin = plate / 4\nlabel = \"rib\"\n".to_string());

        app.read_parameters();

        let names: Vec<&str> = app
            .parameters
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect();
        assert_eq!(names, vec!["plate", "margin"]);
        assert_eq!(app.parameters[0].value(), Some(80.0));
        assert_eq!(app.parameters[1].value(), None);
        assert_eq!(app.parameters[1].expression(), Some("plate / 4"));

        // A model leaving the screen takes its parameters with it.
        app.clear_loaded_model();
        assert!(app.parameters.is_empty());
    }

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
        let message = turn_chat_message(
            "Made a box",
            TurnGeometry::Solid {
                before: None,
                after: &summary(1000.0, 6),
            },
        );
        assert_eq!(
            message,
            "Made a box\n\nModel: 6 faces, volume 1000 mm³, 10 × 10 × 10 mm."
        );
    }

    #[test]
    fn unchanged_edit_reports_measurements_before_and_after() {
        let before = summary(1000.0, 6);
        let same = turn_chat_message(
            "Renamed a parameter",
            TurnGeometry::Solid {
                before: Some(&before),
                after: &before,
            },
        );
        assert_eq!(
            same,
            "Renamed a parameter\n\nModel unchanged. Before: 6 faces, volume 1000 mm³, 10 × 10 × 10 mm. After: 6 faces, volume 1000 mm³, 10 × 10 × 10 mm."
        );
    }

    #[test]
    fn changed_edit_reports_every_measurement_before_and_after() {
        let before = summary(1000.0, 6);
        let message = turn_chat_message(
            "Added a hole",
            TurnGeometry::Solid {
                before: Some(&before),
                after: &summary(900.0, 9),
            },
        );
        assert_eq!(
            message,
            "Added a hole\n\nModel change: faces 6 to 9; volume 1000 to 900 mm³ (-10.0%). Before: 6 faces, volume 1000 mm³, 10 × 10 × 10 mm. After: 9 faces, volume 900 mm³, 10 × 10 × 10 mm."
        );
    }

    #[test]
    fn exactly_two_comment_anchors_become_the_measurement_pair() {
        let anchor = |id| GeometryContext {
            element: TopologyElement::Face(FaceId(id)),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: vec![],
            chosen_candidate: None,
        };
        let first = anchor(1);
        let second = anchor(4);
        assert_eq!(measurement_pair(std::slice::from_ref(&first)), None);
        assert_eq!(
            measurement_pair(&[first.clone(), second.clone()]),
            Some((first.element.clone(), second.element.clone()))
        );
        assert_eq!(measurement_pair(&[first, second, anchor(7)]), None);
    }

    #[test]
    fn single_readout_tracks_the_latest_selection_not_an_earlier_anchor() {
        let descriptors = GeometryDescriptors {
            faces: vec![],
            edges: vec![EdgeDescriptor {
                curve_type: "line".into(),
                length: 12.0,
                radius: None,
                centre: [0.0; 3],
                neighbours: vec![],
            }],
            vertices: vec![],
        };
        assert_eq!(
            measurement_readout(
                &SelectionState::Selected(TopologyElement::Edge(EdgeId(0))),
                None,
                Some(&descriptors),
            ),
            Some("Length 12 mm".into())
        );
        assert_eq!(
            measurement_readout(&SelectionState::None, None, Some(&descriptors)),
            None
        );
    }

    #[tokio::test]
    async fn stored_provider_settings_reach_the_responses_wire_and_sequence_errors_omit_values() {
        let (base_url, server) = recording_provider(
            429,
            serde_json::json!({"error": {"type": "rate_limit_error", "code": "rate_limit_exceeded", "message": "try again later"}}),
        );
        let configuration_dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::at(configuration_dir.path().join("cadmark"));
        store
            .save(&settings_for(base_url, "local-cad-model"))
            .unwrap();
        store.save_credential("test-only-stored-token").unwrap();
        let error = provider_error_from_settings(&store.load().unwrap(), &store).await;
        let request = server.join().unwrap().unwrap();
        assert!(error.to_string().contains("usage limit"));
        assert_eq!(request.path, "/v1/responses");
        assert!(request.authenticated);
        assert_eq!(request.body["model"], "local-cad-model");
        assert_eq!(request.body["stream"], true);

        let wrong_type = "endpoint-value-that-must-not-echo";
        let malformed_dir = configuration_dir.path().join("wrong-type");
        std::fs::create_dir_all(&malformed_dir).unwrap();
        std::fs::write(
            malformed_dir.join("settings.json"),
            format!(r#"{{"ai":{{"base_url":["{wrong_type}"],"model":"m"}}}}"#),
        )
        .unwrap();
        let error = SettingsStore::at(malformed_dir).load().unwrap_err();
        assert!(!error.contains(wrong_type));
    }

    #[test]
    fn a_sketch_faces_its_own_plane_even_when_a_solid_is_already_on_screen() {
        let sketch = cadmark_core::sketch::SketchProfile {
            plane: cadmark_core::sketch::SketchPlane {
                origin: [0.0; 3],
                normal: [1.0, 0.0, 0.0],
                x_axis: [0.0, 1.0, 0.0],
            },
            curves: Vec::new(),
            corners: Vec::new(),
            regions: Vec::new(),
        };
        let profile_bounds = Bounds3::from_positions([[0.0, -5.0, -5.0], [0.0, 5.0, 5.0]]);

        // A solid was built first, so the install reports no fresh
        // framing; the sketch drawn in front of it is still faced and
        // framed on its own plane.
        let (face, frame) = camera_change(Some(&sketch), None, profile_bounds);
        assert_eq!(face, Some([1.0, 0.0, 0.0]));
        assert_eq!(frame, profile_bounds);

        // The first model on screen behaves the same way.
        let (face, frame) = camera_change(Some(&sketch), profile_bounds, profile_bounds);
        assert_eq!(face, Some([1.0, 0.0, 0.0]));
        assert_eq!(frame, profile_bounds);

        // A solid faces nothing in particular, and a rebuild after the
        // first model leaves the user's view alone.
        let solid_bounds = Bounds3::from_positions([[0.0; 3], [10.0; 3]]);
        assert_eq!(camera_change(None, None, solid_bounds), (None, None));
        assert_eq!(
            camera_change(None, solid_bounds, solid_bounds),
            (None, solid_bounds)
        );
    }

    #[test]
    fn hosted_provider_settings_build_the_shared_application_services() {
        let configuration_dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::at(configuration_dir.path().join("cadmark"));
        let settings = UserSettings {
            ai: Some(cadmark_bridge::config::AiConfiguration {
                base_url: "https://hosted.example/v1".to_string(),
                model: "hosted-cad-model".to_string(),
                accepts_images: true,
                allow_insecure_http: false,
            }),
            ..UserSettings::default()
        };
        store.save(&settings).unwrap();
        let services = ai_services(&store.load().unwrap(), Some(&store)).unwrap();
        assert_eq!(services.model.model_name(), "hosted-cad-model");
    }

    #[tokio::test]
    async fn provider_refusals_reach_the_chat_notice_by_cause() {
        let cases = [
            (
                429,
                "rate_limit_error",
                "model_cooldown",
                "usage limit reached",
                "the provider is at its usage limit or cooling down",
            ),
            (
                401,
                "authentication_error",
                "invalid_api_key",
                "credential rejected",
                "the provider rejected the credential",
            ),
            (
                404,
                "invalid_request_error",
                "model_not_found",
                "no such model",
                "the provider does not serve the configured model",
            ),
        ];
        for (status, kind, code, detail, expected_cause) in cases {
            let (base_url, server) = recording_provider(
                status,
                serde_json::json!({"error": {"type": kind, "code": code, "message": detail}}),
            );
            let configuration_dir = tempfile::tempdir().unwrap();
            let store = SettingsStore::at(configuration_dir.path().join("cadmark"));
            store
                .save(&settings_for(base_url, "configured-model"))
                .unwrap();
            store.save_credential("test-only-stored-token").unwrap();
            let error = provider_error_from_settings(&store.load().unwrap(), &store).await;
            server.join().unwrap().unwrap();
            let project_dir = tempfile::tempdir().unwrap();
            let mut app = app_with_pending_response(project_dir.path().to_path_buf());
            app.finish_turn(TurnOutcome::Failed {
                error: error.to_string(),
            });
            let notice = app
                .project()
                .unwrap()
                .conversation
                .messages()
                .last()
                .unwrap();
            assert!(matches!(
                notice.kind,
                MessageKind::Notice { is_error: true }
            ));
            assert!(notice.text.contains(expected_cause));
            assert!(notice.text.contains(detail));
            assert!(!notice.text.contains("test-only-stored-token"));
        }
    }

    fn activity(call_id: &str, tool: &str) -> ToolActivity {
        ToolActivity {
            call_id: call_id.into(),
            tool: tool.into(),
            arguments: serde_json::Value::Null,
            output: None,
            failed: false,
            started: chrono::Utc::now(),
            finished: None,
        }
    }

    fn kinds_and_text(conversation: &Conversation) -> Vec<(MessageKind, String)> {
        conversation
            .messages()
            .iter()
            .map(|message| (message.kind.clone(), message.text.clone()))
            .collect()
    }

    #[test]
    fn what_the_ai_said_before_its_first_tool_call_survives_the_tool_group() {
        let mut conversation = Conversation::new();
        let response = conversation.push(Message::ai_response(""));
        conversation.append_text(response, "Route: sketch — the profile corner");

        let (tools, reply) = record_tool_start(
            &mut conversation,
            None,
            response,
            activity("c1", "run_script"),
        );

        let messages = kinds_and_text(&conversation);
        assert_eq!(messages.len(), 3, "{messages:?}");
        assert_eq!(
            messages[0],
            (
                MessageKind::AiResponse,
                "Route: sketch — the profile corner".to_string()
            ),
            "the announcement must stay above the tool group"
        );
        assert!(matches!(messages[1].0, MessageKind::ToolCalls(_)));
        assert_eq!(messages[2], (MessageKind::AiResponse, String::new()));
        assert_ne!(reply, response, "the turn continues in a fresh reply");
        assert!(conversation.message_mut(tools).is_some());
    }

    #[test]
    fn a_route_stated_mid_turn_sits_above_the_run_it_committed_to() {
        // The model looks something up, then commits to a route, then
        // runs: the commitment must read as made before the run, not
        // folded in beside it.
        let mut conversation = Conversation::new();
        let response = conversation.push(Message::ai_response(""));
        let (tools, response) = record_tool_start(
            &mut conversation,
            None,
            response,
            activity("c1", "lookup_docs"),
        );
        conversation.append_text(response, "Route: solid — the vertical edge of the boss");
        let (later, _) = record_tool_start(
            &mut conversation,
            Some(tools),
            response,
            activity("c2", "run_script"),
        );

        assert_ne!(later, tools, "speaking closes the group it followed");
        let messages = kinds_and_text(&conversation);
        let shape: Vec<_> = messages
            .iter()
            .map(|(kind, text)| match kind {
                MessageKind::ToolCalls(activities) => {
                    format!("tools:{}", activities[0].tool)
                }
                _ => format!("said:{text}"),
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                "tools:lookup_docs".to_string(),
                "said:Route: solid — the vertical edge of the boss".to_string(),
                "tools:run_script".to_string(),
                "said:".to_string(),
            ],
            "{messages:?}"
        );
    }

    #[test]
    fn calls_made_without_speaking_in_between_stay_one_group() {
        let mut conversation = Conversation::new();
        let response = conversation.push(Message::ai_response(""));
        let (tools, response) = record_tool_start(
            &mut conversation,
            None,
            response,
            activity("c1", "lookup_docs"),
        );
        let (again, _) = record_tool_start(
            &mut conversation,
            Some(tools),
            response,
            activity("c2", "run_script"),
        );

        assert_eq!(again, tools);
        let messages = kinds_and_text(&conversation);
        assert_eq!(messages.len(), 2, "{messages:?}");
        let MessageKind::ToolCalls(activities) = &messages[0].0 else {
            panic!("the group is first: {messages:?}");
        };
        assert_eq!(activities.len(), 2);
    }

    #[test]
    fn a_turn_that_said_nothing_first_leaves_no_blank_reply() {
        let mut conversation = Conversation::new();
        let response = conversation.push(Message::ai_response(""));

        record_tool_start(
            &mut conversation,
            None,
            response,
            activity("c1", "run_script"),
        );

        let messages = kinds_and_text(&conversation);
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(matches!(messages[0].0, MessageKind::ToolCalls(_)));
    }

    /// Two candidates recorded against the same elements, plus geometry
    /// only one of them claims — the ordinary fillet-over-a-box shape.
    fn coincident_ledger() -> cadmark_core::ledger::ProvenanceLedger {
        use cadmark_core::ledger::{
            ProvenanceEntry, ProvenanceLedger, ProvenanceRelation, SemanticOperation, SourceRef,
        };
        let entry = |line: u32, operation_id: u64| ProvenanceEntry {
            source: SourceRef {
                line,
                code: format!("line {line}"),
            },
            operation: SemanticOperation::Fillet,
            operation_id,
            relation: ProvenanceRelation::Modified,
        };
        let mut ledger = ProvenanceLedger::new();
        // The shared edge: both candidates claim it.
        ledger
            .record_edge(
                EdgeId(5),
                LedgerValue::Ambiguous(vec![entry(2, 1), entry(9, 4)]),
            )
            .unwrap();
        // A face only the box claims, so the two footprints differ here.
        ledger
            .record_face(FaceId(3), LedgerValue::Resolved(entry(2, 1)))
            .unwrap();
        ledger
    }

    #[test]
    fn a_candidates_highlight_is_every_element_its_own_operation_claims() {
        use cadmark_renderer::picking::encode_picking_id;
        let ledger = coincident_ledger();

        // The box line accounts for its own face and the shared edge.
        assert_eq!(
            candidate_highlight_ids(&ledger, 1),
            vec![
                encode_picking_id(&TopologyElement::Face(FaceId(3))),
                encode_picking_id(&TopologyElement::Edge(EdgeId(5))),
            ]
        );
        // The fillet line accounts for the shared edge alone.
        assert_eq!(
            candidate_highlight_ids(&ledger, 4),
            vec![encode_picking_id(&TopologyElement::Edge(EdgeId(5)))]
        );
        // A line claiming nothing lights nothing, rather than everything.
        assert!(candidate_highlight_ids(&ledger, 77).is_empty());
    }

    #[test]
    fn coincident_candidates_are_reported_rather_than_given_a_manufactured_difference() {
        use cadmark_core::ledger::{
            LedgerValue as LV, ProvenanceEntry, ProvenanceLedger, ProvenanceRelation,
            SemanticOperation, SourceRef,
        };
        let entry = |line: u32, operation_id: u64| ProvenanceEntry {
            source: SourceRef {
                line,
                code: format!("line {line}"),
            },
            operation: SemanticOperation::Fillet,
            operation_id,
            relation: ProvenanceRelation::Modified,
        };
        // The ledger records both candidates against the same single edge
        // and nothing else, so their footprints genuinely coincide.
        let mut ledger = ProvenanceLedger::new();
        ledger
            .record_edge(EdgeId(5), LV::Ambiguous(vec![entry(2, 1), entry(9, 4)]))
            .unwrap();

        assert_eq!(
            candidate_highlight_ids(&ledger, 1),
            candidate_highlight_ids(&ledger, 4),
            "the ledger draws no distinction here and the highlight must not invent one"
        );
        assert!(!candidate_highlight_ids(&ledger, 1).is_empty());
    }

    /// C28's start view and part-name dialog are reached only from this
    /// file. A merge that drops those modules and this file's calls to them
    /// together still compiles, which is how they were lost once already;
    /// driving both from here is what refuses that silently.
    #[test]
    fn the_start_view_still_offers_the_project_folders_the_app_remembers() {
        use cadmark_ui::start_view::{StartAction, StartViewState, show_start_view};

        let remembered = std::path::PathBuf::from("/projects/bracket");
        let ctx = egui::Context::default();
        let mut chosen = StartAction::None;
        // The row's position is discovered by sweeping rather than assumed,
        // so the assertion survives the start view being laid out differently.
        for y in 0..800 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 800.0),
                )),
                ..Default::default()
            };
            // The start view centres a 420-wide column in the window.
            let pointer = egui::pos2(450.0, y as f32);
            input.events.push(egui::Event::PointerMoved(pointer));
            input.events.push(egui::Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            input.events.push(egui::Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
            let mut action = StartAction::None;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    action = show_start_view(
                        ui,
                        StartViewState {
                            recent_projects: std::slice::from_ref(&remembered),
                            notice: None,
                            controls_enabled: true,
                        },
                    );
                });
            });
            if action != StartAction::None {
                chosen = action;
                if matches!(chosen, StartAction::OpenRecent(_)) {
                    break;
                }
            }
        }
        assert_eq!(
            chosen,
            StartAction::OpenRecent(remembered),
            "no row of the start view opens the project the app remembers"
        );
    }

    #[test]
    fn the_part_name_dialog_still_carries_a_typed_name_back_to_the_app() {
        use cadmark_ui::part_name_dialog::PartNameAction;

        let ctx = egui::Context::default();
        let mut dialog = PartNameDialog::default();
        assert_eq!(dialog.show(&ctx), PartNameAction::None);
        assert!(!dialog.is_open(), "a dialog nobody opened is not on screen");

        dialog.open();
        assert!(dialog.is_open());

        let mut settled = PartNameAction::None;
        for _ in 0..8 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 800.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::Text("bracket".to_string()));
            input.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            });
            let mut action = PartNameAction::None;
            let _ = ctx.run(input, |ctx| action = dialog.show(ctx));
            if action != PartNameAction::None {
                settled = action;
                break;
            }
        }
        assert_eq!(
            settled,
            PartNameAction::Save("bracket".to_string()),
            "the name typed into the dialog never reached the app"
        );
    }
}
