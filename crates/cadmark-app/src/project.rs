// A project folder as the application holds it open: the worker thread
// executing its script, its design history, its conversation on disk,
// and the model currently on screen. Everything here is replaced when
// another folder is opened; nothing here touches egui or the GPU.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use cadmark_bridge::AiServices;
use cadmark_bridge::backend::{ProviderUsage, TurnModel};
use cadmark_core::cancellation::CancelFlag;
use cadmark_core::context::{IdentificationStrategy, MeasuredIdentification, NullIdentification};
use cadmark_core::export::ExportFormat;
use cadmark_core::geometry::{
    GeometryDescriptors, ModelSummary, PartId, SolidValidity, TopologyElement,
};
use cadmark_core::ledger::ProvenanceLedger;
use cadmark_core::limits::ExecutionLimits;
use cadmark_core::message::{Conversation, Message};
use cadmark_core::sketch::SketchProfile;
use cadmark_core::sketch_lineage::SketchLineageLedger;
use cadmark_core::version::VersionHistory;
use cadmark_kernel::protocol::{ExecutedModel, ModelFile, ModelForm, SketchResult, SolidResult};
use cadmark_renderer::camera::Bounds3;

use crate::orchestrator::{
    OrchestratorCommand, OrchestratorHandle, OrchestratorResult, spawn_orchestrator,
};
use crate::parts::{self, OpenPart};
use crate::reference_images::load_attachment_bytes;
use crate::turn::{RenderSource, TurnInput};
use crate::validity::{ExportDecision, export_decision, export_warning};

/// Where the conversation is kept, inside the project folder.
const CONVERSATION_FILENAME: &str = ".cadmark/conversation.json";
const CONVERSATION_ARCHIVE_DIRECTORY: &str = ".cadmark/conversations";

/// How often the script on disk is compared with the model on screen.
pub const SCRIPT_WATCH_INTERVAL: Duration = Duration::from_secs(1);

fn load_history(dir: &Path) -> VersionHistory {
    match (
        crate::git_ops::list_microversions(dir, 100),
        crate::git_ops::list_current_lane_microversions(dir, 100),
    ) {
        (Ok(versions), Ok(current_lane)) => VersionHistory::from_history(versions, current_lane),
        (Err(e), _) | (_, Err(e)) => {
            log::warn!("Failed to load microversion history: {e}");
            VersionHistory::new()
        }
    }
}

/// The model on screen: what the application keeps from the last
/// successful execution besides the mesh, which lives on the GPU. A
/// design that has reached only a sketch is here too, and carries no
/// summary, validity or exportable file, because it has none.
pub struct LoadedModel {
    pub descriptors: GeometryDescriptors,
    pub bounds: Option<Bounds3>,
    pub form: ModelForm,
}

impl LoadedModel {
    pub fn solid(&self) -> Option<&SolidResult> {
        match &self.form {
            ModelForm::Solid(solid) => Some(solid),
            ModelForm::Sketch(_) => None,
        }
    }

    pub fn sketch(&self) -> Option<&SketchProfile> {
        self.sketch_result().map(|sketch| &sketch.profile)
    }

    /// The sketch on screen with the file the worker kept for it.
    pub fn sketch_result(&self) -> Option<&SketchResult> {
        match &self.form {
            ModelForm::Sketch(sketch) => Some(sketch),
            ModelForm::Solid(_) => None,
        }
    }

    /// The solid's summary, or nothing for a sketch — there is no volume
    /// or face count to state until the profile becomes a solid.
    pub fn summary(&self) -> Option<&ModelSummary> {
        self.solid().map(|solid| &solid.summary)
    }

    /// The formats the model on screen can be written to: solid formats
    /// for a solid, drawings and STEP for a sketch.
    pub fn export_formats(&self) -> &'static [ExportFormat] {
        match &self.form {
            ModelForm::Solid(_) => &ExportFormat::SOLID,
            ModelForm::Sketch(_) => &ExportFormat::SKETCH,
        }
    }

    /// Per-solid kernel validity, empty for a sketch.
    pub fn validity(&self) -> &[SolidValidity] {
        self.solid().map_or(&[], |solid| solid.validity.as_slice())
    }
}

/// One completed part of the model on screen. Part IDs are the worker's
/// ordered execution identities; names are the script bindings shown to the
/// user. Each part carries its own provenance and topology tables, because
/// face, edge and vertex numbering restarts within every part.
pub struct LoadedPart {
    pub id: u32,
    pub name: String,
    pub summary: ModelSummary,
    pub ledger: ProvenanceLedger,
    pub sketch_lineage: SketchLineageLedger,
    pub descriptors: GeometryDescriptors,
    pub model: ModelFile,
    /// Per-solid kernel validity retained for the export gate.
    pub validity: Vec<SolidValidity>,
}

/// What the worker thread is doing, for the status bar and chat.
#[derive(Debug, Clone)]
pub enum Busy {
    /// An AI turn is running; the flag ends it early.
    Turn {
        cancel: CancelFlag,
        started: Instant,
        last_event: Instant,
        phase: String,
    },
    /// Executing the script on disk.
    Building,
}

impl Busy {
    pub fn label(&self) -> String {
        match self {
            Self::Turn { phase, .. } => phase.clone(),
            Self::Building => "Building model\u{2026}".to_string(),
        }
    }
}

/// An open project folder.
pub struct Project {
    pub dir: PathBuf,
    pub ai_accepts_images: bool,
    /// The part of the folder currently being modelled.
    part: OpenPart,
    /// Every part script in the folder, for the switcher.
    parts: Vec<String>,
    /// The worker thread and its channels. Taken by `shut_down`, after
    /// which the project sends nothing further.
    orchestrator: Option<OrchestratorHandle>,
    pub history: VersionHistory,
    pub conversation: Conversation,
    /// The AI model in use, for the toolbar badge; absent when AI is
    /// unavailable and the reason is in the conversation.
    pub ai_model: Option<String>,
    /// The model's context window as the endpoint advertised it at open,
    /// which takes precedence over the manual setting; absent when the
    /// endpoint advertised none.
    pub detected_context_window: Option<usize>,
    /// What the provider reported the last request of a turn cost; the
    /// one measured figure the occupancy estimate is shown against.
    pub last_usage: Option<ProviderUsage>,
    pub busy: Option<Busy>,
    /// Provenance ledger — rebuilt on each script execution.
    pub ledger: ProvenanceLedger,
    /// Which sketch curve drew each element, or the stated reason none can
    /// be named. Rebuilt with the ledger, never persisted.
    pub sketch_lineage: SketchLineageLedger,
    /// Identification strategy for geometry context, rebuilt from the
    /// measured geometry of each executed model.
    pub identification: Box<dyn IdentificationStrategy>,
    /// The model on screen, if a script has executed successfully.
    pub model: Option<LoadedModel>,
    /// Every part the executed script defines, in execution order.
    pub model_parts: Vec<LoadedPart>,
    /// The part whose face, edge and vertex IDs the current ledger and
    /// descriptors belong to.
    pub active_model_part_id: Option<u32>,
    /// The source that produced the model on screen.
    pub script_source: Option<String>,
    /// Whether the script exists on disk, whether or not it runs.
    pub has_script: bool,
    /// Modification time of the script when the model was last built, and
    /// when it was last compared with disk.
    script_mtime: Option<SystemTime>,
    script_checked_at: Instant,
    /// Whether the script on disk differs from the model on screen.
    pub script_modified_on_disk: bool,
    pub exports_in_flight: usize,
    /// Distance requests waiting on the retained worker model.
    pub measurements_in_flight: usize,
    /// Parts of the open script the user has hidden from the viewport, by
    /// script binding name: the one identity a part keeps across rebuilds,
    /// so a part stays hidden while the AI works on the others. The set
    /// belongs to the script on screen and is emptied when another opens.
    pub hidden_parts: HashSet<String>,
}

impl Project {
    /// Reload both the complete history listing and the lane at the checked-out step.
    pub fn reload_history(&mut self) {
        self.history = load_history(&self.dir);
    }

    /// Open a folder: initialise its git repository, load its history and
    /// conversation, start a worker, and ask for the script to be built.
    pub fn open(
        dir: PathBuf,
        preferred_part: Option<&str>,
        ai: Result<AiServices, String>,
        limits: ExecutionLimits,
        render: Box<dyn RenderSource>,
    ) -> Self {
        let dir = dir.canonicalize().unwrap_or(dir);
        let part = OpenPart::for_folder(&dir, preferred_part);

        if let Err(e) = crate::git_ops::ensure_repo(&dir) {
            log::error!("Failed to initialise git repo in {}: {e}", dir.display());
        }

        let mut conversation = load_conversation(&dir);
        if let Ok(services) = &ai {
            conversation = conversation.for_model(services.model.session_identity().as_deref());
        }
        if conversation.is_empty() {
            conversation.push(Message::notice(format!(
                "Describe what you'd like to build, or click a face, edge or vertex of the \
                     model to comment on it. Every completed turn is saved to {} in the \
                     project folder and recorded as a design step.",
                part.file_name()
            )));
        }
        let ai_accepts_images = ai
            .as_ref()
            .is_ok_and(|services| services.model.accepts_images());
        let ai_model = match &ai {
            Ok(services) => Some(services.model.model_name().to_string()),
            Err(reason) => {
                log::warn!("{reason}");
                conversation.push(Message::notice(format!(
                    "AI is unavailable: {reason}. The model still loads, and you can edit {} \
                     by hand and press Rebuild.",
                    part.file_name()
                )));
                None
            }
        };
        let orchestrator = spawn_orchestrator(
            dir.clone(),
            part.file_name().to_string(),
            ai,
            limits,
            render,
        );

        let history = load_history(&dir);

        let mut project = Self {
            ai_accepts_images,
            parts: parts::list_parts(&dir),
            part,
            dir,
            orchestrator: Some(orchestrator),
            history,
            conversation,
            ai_model,
            detected_context_window: None,
            last_usage: None,
            busy: None,
            ledger: ProvenanceLedger::new(),
            sketch_lineage: SketchLineageLedger::new(),
            identification: Box::new(NullIdentification),
            model: None,
            model_parts: Vec::new(),
            active_model_part_id: None,
            script_source: None,
            has_script: false,
            script_mtime: None,
            script_checked_at: Instant::now(),
            script_modified_on_disk: false,
            exports_in_flight: 0,
            measurements_in_flight: 0,
            hidden_parts: HashSet::new(),
        };
        project.request_reload();
        project
    }

    pub fn script_path(&self) -> PathBuf {
        self.dir.join(self.part.file_name())
    }

    /// The part now open.
    pub fn part(&self) -> &OpenPart {
        &self.part
    }

    /// The file name of the part now open.
    pub fn part_file_name(&self) -> &str {
        self.part.file_name()
    }

    /// Every part script in the folder, most recently listed.
    pub fn parts(&self) -> &[String] {
        &self.parts
    }

    /// Re-read which parts the folder holds.
    pub fn refresh_parts(&mut self) {
        self.parts = parts::list_parts(&self.dir);
    }

    /// Open another part of the same folder: the worker changes script,
    /// the model is rebuilt, and the conversation and history stay as they
    /// are — they belong to the folder, not to one part.
    pub fn switch_part(&mut self, part: OpenPart) {
        if part == self.part {
            return;
        }
        self.part = part;
        self.refresh_parts();
        self.script_source = None;
        self.has_script = false;
        self.script_modified_on_disk = false;
        self.hidden_parts.clear();
        let file_name = self.part.file_name().to_string();
        if self.send(OrchestratorCommand::SetScript(file_name)).is_ok() {
            self.request_reload();
        }
    }

    /// Begin a part with no name of its own. It is written and executed
    /// like any other part; the first save asks for its name.
    pub fn begin_untitled_part(&mut self) -> Result<(), String> {
        let path = self.dir.join(parts::UNTITLED_PART);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|error| format!("Could not clear the previous untitled part: {error}"))?;
        }
        self.switch_part(OpenPart::Untitled);
        // `switch_part` returns early when the part is already open, which
        // it is when a second new part follows an unsaved one.
        self.part = OpenPart::Untitled;
        self.refresh_parts();
        Ok(())
    }

    /// Give the untitled part the name the user typed: the file is renamed
    /// and the part is open under its own name from then on. Refused while
    /// the part already has a name, and refused for a name that cannot be
    /// a file name in this folder.
    pub fn name_untitled_part(&mut self, typed: &str) -> Result<String, String> {
        if self.part.is_named() {
            return Err("This part already has a name".to_string());
        }
        let file_name = parts::name_untitled_part(&self.dir, typed)?;
        // The untitled file may already be in the history; stage the move
        // so the next step records a rename, not a second file.
        if let Err(error) =
            crate::git_ops::stage_part_rename(&self.dir, parts::UNTITLED_PART, &file_name)
        {
            log::warn!("Could not stage the part rename: {error}");
        }
        self.part = OpenPart::Named(file_name.clone());
        self.refresh_parts();
        let _ = self.send(OrchestratorCommand::SetScript(file_name.clone()));
        self.record_script_state();
        self.has_script = self.script_path().exists();
        Ok(file_name)
    }

    /// Replace the worker with a channel the test holds, so the command
    /// a path sends can be read back off it.
    #[cfg(test)]
    pub(crate) fn stand_in_worker(&mut self) -> mpsc::Receiver<OrchestratorCommand> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (_result_tx, result_rx) = mpsc::channel();
        self.orchestrator = Some(OrchestratorHandle {
            commands: cmd_tx,
            results: result_rx,
            thread: std::thread::spawn(|| {}),
        });
        cmd_rx
    }

    /// Replace the worker with a channel supplying results to the real polling path.
    #[cfg(test)]
    pub(crate) fn stand_in_worker_results(&mut self) -> mpsc::Sender<OrchestratorResult> {
        let (cmd_tx, _cmd_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        self.orchestrator = Some(OrchestratorHandle {
            commands: cmd_tx,
            results: result_rx,
            thread: std::thread::spawn(|| {}),
        });
        result_tx
    }

    /// Hand a command to the worker.
    pub fn send(&mut self, command: OrchestratorCommand) -> Result<(), String> {
        self.orchestrator
            .as_ref()
            .ok_or(())
            .and_then(|orchestrator| orchestrator.commands.send(command).map_err(|_| ()))
            .map_err(|()| "The modelling worker has stopped; restart CADmark".to_string())
    }

    /// Stop the worker thread and wait for it: any running turn is
    /// cancelled, the command channel is closed so the thread's loop ends,
    /// and the thread is joined. Ordering the teardown this way keeps the
    /// thread from outliving the GPU device and runtime it renders and
    /// executes with, which the process would otherwise tear down under
    /// it. Safe to call more than once.
    pub fn shut_down(&mut self) {
        self.cancel_turn();
        self.busy = None;
        let Some(orchestrator) = self.orchestrator.take() else {
            return;
        };
        let OrchestratorHandle {
            commands,
            results,
            thread,
        } = orchestrator;
        drop(commands);
        // A worker blocked sending a result must not deadlock the join:
        // keep draining until the thread is gone.
        while !thread.is_finished() {
            match results.recv_timeout(Duration::from_millis(50)) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if thread.join().is_err() {
            log::warn!("The modelling worker thread ended with a panic");
        }
    }

    /// Ask the worker to re-read and execute the script on disk.
    pub fn request_reload(&mut self) {
        if self.send(OrchestratorCommand::Reload).is_ok() {
            self.busy = Some(Busy::Building);
        }
    }

    /// Start an AI turn. `history` is the conversation as it stood before
    /// this turn's own messages were recorded: the worker shows the model
    /// that history and then the turn's input, so the request is not
    /// shown twice.
    pub fn start_turn(
        &mut self,
        input: TurnInput,
        history: Conversation,
    ) -> Result<CancelFlag, String> {
        let cancel = CancelFlag::new();
        self.send(OrchestratorCommand::Turn {
            input,
            conversation: history,
            cancel: cancel.clone(),
        })?;
        let now = Instant::now();
        self.busy = Some(Busy::Turn {
            cancel: cancel.clone(),
            started: now,
            last_event: now,
            phase: "starting".to_string(),
        });
        Ok(cancel)
    }

    /// End the running turn early. The worker restores the script and
    /// reports the cancellation like any other outcome.
    pub fn cancel_turn(&self) {
        if let Some(Busy::Turn { cancel, .. }) = &self.busy {
            cancel.cancel();
        }
    }

    /// Note progress on the running turn.
    pub fn note_turn_event(&mut self, phase: Option<String>) {
        if let Some(Busy::Turn {
            last_event,
            phase: current,
            ..
        }) = &mut self.busy
        {
            *last_event = Instant::now();
            if let Some(phase) = phase {
                *current = phase;
            }
        }
    }

    /// Write the current model next to the script: a solid in a solid
    /// format, a sketch as a drawing in its own plane or as STEP. The part
    /// last selected is what a multi-part model writes, so the file
    /// matches every visible cue about which part is chosen.
    pub fn request_export(&mut self, format: ExportFormat) -> Result<PathBuf, String> {
        let model = self.model.as_ref().ok_or("no model to export")?;
        if !model.export_formats().contains(&format) {
            return Err(format!(
                "{} is not a format {} can be written to",
                format.label(),
                match &model.form {
                    ModelForm::Solid(_) => "a solid",
                    ModelForm::Sketch(_) => "a sketch",
                }
            ));
        }
        let (file, plane) = match &model.form {
            ModelForm::Sketch(sketch) => (sketch.file.clone(), Some(sketch.profile.plane)),
            ModelForm::Solid(solid) => {
                if let Some(id) = self.active_model_part_id
                    && self.model_parts.len() > 1
                {
                    return self.request_part_export(id, format);
                }
                let decision = export_decision(&solid.validity);
                if decision != ExportDecision::Ready {
                    return Err(export_warning(&decision).expect("non-ready decision has warning"));
                }
                (solid.file.clone(), None)
            }
        };
        let stem = parts::part_display_name(self.part.file_name());
        let path = self.dir.join(format!("{stem}.{}", format.extension()));
        self.send(OrchestratorCommand::Export {
            model: file,
            format,
            path: path.clone(),
            plane,
        })?;
        self.exports_in_flight += 1;
        Ok(path)
    }

    /// Write one part of the model next to the script, from the BREP the
    /// worker retained for it, without re-running the script.
    pub fn request_part_export(
        &mut self,
        id: u32,
        format: ExportFormat,
    ) -> Result<PathBuf, String> {
        let part = self
            .model_parts
            .iter()
            .find(|part| part.id == id)
            .ok_or("selected part no longer exists")?;
        let decision = export_decision(&part.validity);
        if decision != ExportDecision::Ready {
            return Err(export_warning(&decision).expect("non-ready decision has warning"));
        }
        let (model, name) = (part.model.clone(), part.name.clone());
        let stem = parts::part_display_name(self.part.file_name());
        let path = self
            .dir
            .join(format!("{stem}-{name}.{}", format.extension()));
        self.send(OrchestratorCommand::Export {
            model,
            format,
            path: path.clone(),
            plane: None,
        })?;
        self.exports_in_flight += 1;
        Ok(path)
    }

    /// The part whose ledger, lineage and descriptors the current selection
    /// resolves against; none until a solid with parts is loaded.
    pub fn active_model_part(&self) -> Option<&LoadedPart> {
        let id = self.active_model_part_id?;
        self.model_parts.iter().find(|part| part.id == id)
    }

    /// The ledger a part's elements are numbered in; with no part named,
    /// the ledger the current selection resolves against.
    pub fn ledger_of(&self, part: Option<PartId>) -> &ProvenanceLedger {
        part.and_then(|PartId(id)| self.model_parts.iter().find(|part| part.id == id))
            .map_or(&self.ledger, |part| &part.ledger)
    }

    /// Make one part's ledger, lineage and descriptors the ones later picks
    /// resolve against: the part the user last clicked, in the viewport or
    /// the parts list.
    pub fn select_model_part(&mut self, id: u32) -> Option<&LoadedPart> {
        let part = self.model_parts.iter().find(|part| part.id == id)?;
        self.ledger = part.ledger.clone();
        self.sketch_lineage = part.sketch_lineage.clone();
        self.identification = Box::new(MeasuredIdentification {
            descriptors: part.descriptors.clone(),
        });
        self.active_model_part_id = Some(id);
        self.model_parts.iter().find(|part| part.id == id)
    }

    /// The retained BREP two picked elements are measured on: the part
    /// they are numbered within, since every part numbers its own
    /// elements, or the whole solid where the picks name no part.
    pub fn measurement_model(&self, part: Option<PartId>) -> Result<ModelFile, String> {
        let model = self.model.as_ref().ok_or("no model to measure")?;
        let solid = model
            .solid()
            .ok_or("a sketch has no solid to measure between")?;
        match part {
            Some(PartId(id)) => self
                .model_parts
                .iter()
                .find(|part| part.id == id)
                .map(|part| part.model.clone())
                .ok_or_else(|| "the measured part is no longer loaded".to_string()),
            None => Ok(solid.file.clone()),
        }
    }

    /// Ask the retained worker model for the closest separation of two
    /// picked elements, both numbered within `part`.
    pub fn request_minimum_distance(
        &mut self,
        part: Option<PartId>,
        first: TopologyElement,
        second: TopologyElement,
    ) -> Result<(), String> {
        let model = self.measurement_model(part)?;
        self.send(OrchestratorCommand::MinimumDistance {
            model,
            first,
            second,
        })?;
        self.measurements_in_flight += 1;
        Ok(())
    }

    pub fn set_limits(&mut self, limits: ExecutionLimits) {
        let _ = self.send(OrchestratorCommand::SetLimits(limits));
    }

    /// Non-blocking: the results the worker has produced since last asked.
    pub fn poll(&mut self) -> Vec<OrchestratorResult> {
        let mut results = Vec::new();
        if let Some(orchestrator) = &self.orchestrator {
            while let Ok(result) = orchestrator.results.try_recv() {
                results.push(result);
            }
        }
        results
    }

    /// Take a freshly executed model on screen. Returns the bounds to
    /// frame when this is the first model.
    pub fn install_model(&mut self, model: ExecutedModel, source: String) -> Option<Bounds3> {
        let first_model = self.model.is_none();
        log::info!(
            "Model ready: {} vertices, {} provenance entries",
            model.mesh.vertices.len(),
            model.ledger.len(),
        );
        // A sketch has no mesh, so its own points are what the camera
        // frames; a solid is framed across every part it defines.
        let bounds = match &model.form {
            ModelForm::Sketch(sketch) => Bounds3::from_positions(sketch.profile.points()),
            ModelForm::Solid(solid) => Bounds3::from_positions(
                solid
                    .parts
                    .iter()
                    .flat_map(|part| part.mesh.vertices.iter().map(|vertex| vertex.position)),
            ),
        };
        self.model_parts = model
            .solid()
            .map(|solid| {
                solid
                    .parts
                    .iter()
                    .map(|part| LoadedPart {
                        id: part.id,
                        name: part.name.clone(),
                        summary: part.summary.clone(),
                        ledger: part.ledger.clone(),
                        sketch_lineage: part.sketch_lineage.clone(),
                        descriptors: part.descriptors.clone(),
                        model: part.file.clone(),
                        validity: part.validity.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.active_model_part_id = self.model_parts.last().map(|part| part.id);
        self.ledger = model.ledger;
        self.sketch_lineage = model.sketch_lineage;
        self.identification = Box::new(MeasuredIdentification {
            descriptors: model.descriptors.clone(),
        });
        self.model = Some(LoadedModel {
            descriptors: model.descriptors,
            bounds,
            form: model.form,
        });
        self.script_source = Some(source);
        self.has_script = true;
        self.record_script_state();
        first_model.then_some(bounds).flatten()
    }

    /// Forget the model on screen so a failed reload shows nothing stale.
    pub fn clear_model(&mut self) {
        self.ledger.clear();
        self.sketch_lineage = SketchLineageLedger::new();
        self.identification = Box::new(NullIdentification);
        self.model = None;
        self.model_parts.clear();
        self.active_model_part_id = None;
    }

    /// Persist the conversation beside the script. The file is replaced
    /// whole, so a failure part-way leaves the last saved conversation in
    /// place rather than a truncated one; the failure is returned for the
    /// caller to show, since a chat that silently stops saving is lost only
    /// when the project is next opened.
    pub fn save_conversation(&self) -> Result<(), String> {
        let path = self.dir.join(CONVERSATION_FILENAME);
        let contents = serde_json::to_string(&self.conversation).expect("conversation serialises");
        write_replacing(&path, contents.as_bytes())
            .map_err(|error| format!("Could not save the conversation: {error}"))
    }

    /// Archive the conversation before beginning a blank one. The script is
    /// intentionally untouched: it remains the project's source of truth.
    pub fn start_fresh_conversation(&mut self) -> Result<(), String> {
        fresh_conversation(&self.dir, &mut self.conversation)?;
        self.save_conversation()
    }

    /// Remember the script's modification time so later edits on disk can
    /// be noticed.
    pub fn record_script_state(&mut self) {
        self.script_mtime = std::fs::metadata(self.script_path())
            .and_then(|metadata| metadata.modified())
            .ok();
        self.script_modified_on_disk = false;
        self.script_checked_at = Instant::now();
    }

    /// Compare the script on disk with the model on screen, at most once
    /// per watch interval.
    pub fn watch_script(&mut self) {
        if !self.has_script || self.script_checked_at.elapsed() < SCRIPT_WATCH_INTERVAL {
            return;
        }
        self.script_checked_at = Instant::now();
        let on_disk = std::fs::metadata(self.script_path())
            .and_then(|metadata| metadata.modified())
            .ok();
        self.script_modified_on_disk = on_disk != self.script_mtime;
    }
}

/// Write `contents` to `path` through a sibling temporary file renamed into
/// place, so the file at `path` is at every instant either its previous
/// contents or the new ones.
fn write_replacing(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let staged = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&staged)?;
        std::io::Write::write_all(&mut file, contents)?;
        file.sync_all()?;
        std::fs::rename(&staged, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    written
}

/// The saved conversation with its attached images loaded from the
/// attachment store, so replaying it shows the model every picture again.
fn load_conversation(dir: &Path) -> Conversation {
    let mut conversation: Conversation =
        match std::fs::read_to_string(dir.join(CONVERSATION_FILENAME)) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|error| {
                log::warn!("Ignoring unreadable conversation: {error}");
                Conversation::new()
            }),
            Err(_) => Conversation::new(),
        };
    for message in conversation.messages_mut() {
        load_attachment_bytes(dir, &mut message.attachments);
    }
    conversation
}

fn archive_conversation(dir: &Path, conversation: &Conversation) -> Result<PathBuf, String> {
    let archive_dir = dir.join(CONVERSATION_ARCHIVE_DIRECTORY);
    std::fs::create_dir_all(&archive_dir)
        .map_err(|error| format!("Could not create the conversation archive: {error}"))?;
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| format!("Could not name the conversation archive: {error}"))?
        .as_nanos();
    let path = archive_dir.join(format!("conversation-{timestamp}.json"));
    let contents = serde_json::to_string(conversation)
        .map_err(|error| format!("Could not archive the conversation: {error}"))?;
    std::fs::write(&path, contents)
        .map_err(|error| format!("Could not archive the conversation: {error}"))?;
    Ok(path)
}

fn fresh_conversation(dir: &Path, conversation: &mut Conversation) -> Result<PathBuf, String> {
    let archive = archive_conversation(dir, conversation)?;
    *conversation = Conversation::new();
    conversation.push(Message::notice(
        "Started a new conversation. The earlier conversation is saved in .cadmark/conversations; the part scripts are unchanged.",
    ));
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sketch_form() -> ModelForm {
        ModelForm::Sketch(SketchResult::new(
            SketchProfile {
                plane: cadmark_core::sketch::SketchPlane {
                    origin: [0.0; 3],
                    normal: [0.0, 1.0, 0.0],
                    x_axis: [1.0, 0.0, 0.0],
                },
                ..SketchProfile::default()
            },
            ModelFile(PathBuf::from("/scratch/sketch-1.brep")),
        ))
    }

    #[test]
    fn a_sketch_on_screen_offers_no_solid_to_export_or_measure() {
        let sketch = LoadedModel {
            descriptors: GeometryDescriptors::default(),
            bounds: None,
            form: sketch_form(),
        };
        let solid = LoadedModel {
            descriptors: GeometryDescriptors::default(),
            bounds: None,
            form: ModelForm::Solid(SolidResult {
                validity: vec![SolidValidity {
                    closed: true,
                    valid: true,
                    ..SolidValidity::default()
                }],
                ..SolidResult::new(
                    ModelSummary {
                        volume: 1000.0,
                        bounds_max: [10.0; 3],
                        face_count: 6,
                        edge_count: 12,
                        vertex_count: 8,
                        ..ModelSummary::default()
                    },
                    ModelFile(PathBuf::from("/scratch/model-1.brep")),
                )
            }),
        };

        assert!(sketch.solid().is_none());
        assert!(sketch.sketch().is_some());
        assert!(sketch.summary().is_none());
        assert!(sketch.validity().is_empty());
        assert_eq!(sketch.export_formats(), &ExportFormat::SKETCH);

        assert!(solid.sketch().is_none());
        assert_eq!(solid.summary().map(|summary| summary.face_count), Some(6));
        assert_eq!(solid.validity().len(), 1);
        assert_eq!(solid.export_formats(), &ExportFormat::SOLID);
    }

    /// A project with no worker behind it: the receiver stands in for the
    /// orchestrator so a command can be read back off the channel.
    fn project_for_test(dir: PathBuf) -> (Project, mpsc::Receiver<OrchestratorCommand>) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (_result_tx, result_rx) = mpsc::channel();
        let project = Project {
            ai_accepts_images: false,
            parts: Vec::new(),
            part: OpenPart::Named("bracket.py".to_string()),
            dir,
            orchestrator: Some(OrchestratorHandle {
                commands: cmd_tx,
                results: result_rx,
                thread: std::thread::spawn(|| {}),
            }),
            history: VersionHistory::new(),
            conversation: Conversation::new(),
            ai_model: None,
            detected_context_window: None,
            last_usage: None,
            busy: None,
            ledger: ProvenanceLedger::new(),
            sketch_lineage: SketchLineageLedger::new(),
            identification: Box::new(NullIdentification),
            model: None,
            model_parts: Vec::new(),
            active_model_part_id: None,
            script_source: None,
            has_script: false,
            script_mtime: None,
            script_checked_at: Instant::now(),
            script_modified_on_disk: false,
            exports_in_flight: 0,
            measurements_in_flight: 0,
            hidden_parts: HashSet::new(),
        };
        (project, cmd_rx)
    }

    #[test]
    fn reopening_with_a_different_model_discards_opaque_context_before_display() {
        let dir = tempfile::tempdir().unwrap();
        let (mut project, _commands) = project_for_test(dir.path().to_path_buf());
        project
            .conversation
            .push(Message::user_chat("Keep the design"));
        project.conversation.record_session_for(
            vec![cadmark_core::model_session::ModelItem::ProviderOutput(
                serde_json::json!({
                    "type": "reasoning", "encrypted_content": "x".repeat(4000)
                }),
            )],
            Some("another backend".into()),
        );
        project.save_conversation().unwrap();
        drop(project);
        let services = cadmark_bridge::build_ai_services(
            cadmark_bridge::config::AiConfiguration {
                base_url: "http://127.0.0.1:9/v1".into(),
                model: "new-model".into(),
                accepts_images: false,
                allow_insecure_http: true,
                reasoning_effort: None,
            },
            None,
        )
        .unwrap();
        let mut reopened = Project::open(
            dir.path().to_path_buf(),
            None,
            Ok(services),
            ExecutionLimits::default(),
            Box::new(crate::turn::NoRender),
        );
        assert!(reopened.conversation.session().items.is_empty());
        assert_eq!(reopened.conversation.messages()[0].text, "Keep the design");
        assert!(reopened.conversation.estimated_tokens() < 1000);
        reopened.shut_down();
    }

    #[test]
    fn attached_images_come_back_with_their_messages_when_the_project_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = Vec::new();
        image::RgbImage::from_pixel(48, 24, image::Rgb([20, 80, 160]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let staged = crate::reference_images::stage_bytes("Flange".into(), bytes.clone()).unwrap();
        let attachment = crate::reference_images::store_attachment(dir.path(), &staged).unwrap();
        let (mut project, _commands) = project_for_test(dir.path().to_path_buf());
        project
            .conversation
            .push(Message::user_chat("match this").with_attachments(vec![attachment.clone()]));
        project.save_conversation().unwrap();
        let saved = std::fs::read_to_string(dir.path().join(CONVERSATION_FILENAME)).unwrap();
        assert!(saved.contains(&attachment.file));
        assert!(
            !saved.contains("\"bytes\""),
            "the conversation file holds the name, not the bytes"
        );
        drop(project);

        let reopened = load_conversation(dir.path());
        let message = &reopened.messages()[0];
        assert_eq!(message.attachments[0].name, "Flange");
        assert_eq!(message.attachments[0].bytes, bytes);

        // A file gone from the store leaves the message naming the image
        // without any bytes to send.
        std::fs::remove_file(
            dir.path()
                .join(".cadmark/attachments")
                .join(&attachment.file),
        )
        .unwrap();
        let reopened = load_conversation(dir.path());
        assert_eq!(reopened.messages()[0].attachments[0].name, "Flange");
        assert!(reopened.messages()[0].images().is_empty());
    }

    fn part_for_export_test(model: ModelFile, validity: Vec<SolidValidity>) -> LoadedPart {
        LoadedPart {
            id: 7,
            name: "bracket".to_string(),
            summary: ModelSummary {
                volume: 1.0,
                bounds_max: [1.0; 3],
                face_count: 1,
                edge_count: 1,
                vertex_count: 1,
                ..ModelSummary::default()
            },
            ledger: ProvenanceLedger::new(),
            sketch_lineage: SketchLineageLedger::new(),
            descriptors: GeometryDescriptors::default(),
            model,
            validity,
        }
    }

    #[test]
    fn a_conversation_saved_in_the_project_is_there_when_it_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("make a bracket"));
        conversation.push(Message::ai_response("Made a bracket."));
        let path = dir.path().join(CONVERSATION_FILENAME);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(&conversation).unwrap()).unwrap();

        assert_eq!(load_conversation(dir.path()), conversation);
        std::fs::write(&path, "not json").unwrap();
        assert!(load_conversation(dir.path()).is_empty());
        assert!(load_conversation(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn shutting_down_joins_the_worker_thread_and_ends_further_commands() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bracket.py"), "width = 10\n").unwrap();
        let mut project = Project::open(
            dir.path().to_path_buf(),
            Some("bracket.py"),
            Err("no provider".to_string()),
            ExecutionLimits::default(),
            Box::new(crate::turn::NoRender),
        );
        assert!(project.orchestrator.is_some());

        project.shut_down();

        assert!(project.orchestrator.is_none());
        assert!(project.busy.is_none());
        assert!(project.send(OrchestratorCommand::Reload).is_err());
        assert!(project.poll().is_empty());
        // Shutting down again is a no-op rather than a panic.
        project.shut_down();
    }

    #[test]
    fn a_failed_save_keeps_the_last_saved_conversation_and_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::open(
            dir.path().to_path_buf(),
            None,
            Err("no provider".to_string()),
            ExecutionLimits::default(),
            Box::new(crate::turn::NoRender),
        );
        project.conversation.push(Message::user_chat("first"));
        project.save_conversation().unwrap();
        let saved = std::fs::read_to_string(dir.path().join(CONVERSATION_FILENAME)).unwrap();
        assert!(
            !dir.path()
                .join(".cadmark")
                .read_dir()
                .unwrap()
                .any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".tmp")
                })
        );

        // Make the destination unwritable: a directory in the file's place
        // cannot be renamed over, so the replacement fails after staging.
        let path = dir.path().join(CONVERSATION_FILENAME);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep"), "x").unwrap();
        project.conversation.push(Message::user_chat("second"));
        let error = project.save_conversation().unwrap_err();
        assert!(
            error.starts_with("Could not save the conversation"),
            "{error}"
        );
        std::fs::remove_dir_all(&path).unwrap();
        std::fs::write(&path, &saved).unwrap();
        assert_eq!(
            load_conversation(dir.path()).messages().len(),
            project.conversation.messages().len() - 1,
            "the earlier save is what remains"
        );
    }

    #[test]
    fn fresh_conversation_archives_chat_without_touching_the_script() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join(parts::DEFAULT_PART);
        std::fs::write(&script, "part = Box(10, 10, 10)").unwrap();
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("Keep the wall at 5 mm."));
        let earlier = conversation.clone();

        let archive = fresh_conversation(dir.path(), &mut conversation).unwrap();

        assert_eq!(
            serde_json::from_str::<Conversation>(&std::fs::read_to_string(archive).unwrap())
                .unwrap(),
            earlier
        );
        assert_eq!(conversation.len(), 1);
        assert!(
            conversation.messages()[0]
                .text
                .contains("Started a new conversation")
        );
        assert_eq!(
            std::fs::read_to_string(script).unwrap(),
            "part = Box(10, 10, 10)"
        );
    }

    #[test]
    fn exporting_a_part_sends_the_brep_the_worker_retained_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut project, cmd_rx) = project_for_test(dir.path().to_path_buf());
        let kept_brep = ModelFile(PathBuf::from("/worker-scratch/bracket.brep"));
        project.model_parts.push(part_for_export_test(
            kept_brep.clone(),
            vec![SolidValidity {
                valid: true,
                closed: true,
                ..SolidValidity::default()
            }],
        ));

        let path = project.request_part_export(7, ExportFormat::Stl).unwrap();

        assert_eq!(project.exports_in_flight, 1);
        match cmd_rx.recv().unwrap() {
            OrchestratorCommand::Export {
                model,
                format,
                path: command_path,
                plane,
            } => {
                assert_eq!(model, kept_brep);
                assert_eq!(format, ExportFormat::Stl);
                assert_eq!(command_path, path);
                assert_eq!(plane, None);
            }
            _ => panic!("part export must send an export command"),
        }
    }

    /// The top-level export of a multi-part model writes the part the
    /// user last selected — the one every cue on screen names — not the
    /// last part the script happened to bind.
    #[test]
    fn exporting_a_multi_part_model_writes_the_selected_part() {
        let dir = tempfile::tempdir().unwrap();
        let (mut project, cmd_rx) = project_for_test(dir.path().to_path_buf());
        let valid = vec![SolidValidity {
            valid: true,
            closed: true,
            ..SolidValidity::default()
        }];
        let mut box_part = part_for_export_test(
            ModelFile(PathBuf::from("/worker-scratch/box.brep")),
            valid.clone(),
        );
        box_part.id = 0;
        box_part.name = "box".to_string();
        let mut lid = part_for_export_test(
            ModelFile(PathBuf::from("/worker-scratch/lid.brep")),
            valid.clone(),
        );
        lid.id = 1;
        lid.name = "lid".to_string();
        project.model_parts = vec![box_part, lid];
        project.model = Some(LoadedModel {
            descriptors: GeometryDescriptors::default(),
            bounds: None,
            form: ModelForm::Solid(SolidResult {
                validity: valid,
                ..SolidResult::new(
                    ModelSummary {
                        volume: 1.0,
                        bounds_max: [1.0; 3],
                        face_count: 6,
                        edge_count: 12,
                        vertex_count: 8,
                        ..ModelSummary::default()
                    },
                    ModelFile(PathBuf::from("/worker-scratch/lid.brep")),
                )
            }),
        });
        project.select_model_part(0);

        let path = project.request_export(ExportFormat::Stl).unwrap();

        assert_eq!(path, dir.path().join("bracket-box.stl"));
        match cmd_rx.recv().unwrap() {
            OrchestratorCommand::Export { model, .. } => {
                assert_eq!(model, ModelFile(PathBuf::from("/worker-scratch/box.brep")));
            }
            _ => panic!("export must send an export command"),
        }
    }

    /// A sketch exports as a drawing on its own plane, from the file the
    /// worker kept for it; a solid format is refused by name.
    #[test]
    fn exporting_a_sketch_sends_its_kept_file_and_plane() {
        let dir = tempfile::tempdir().unwrap();
        let (mut project, cmd_rx) = project_for_test(dir.path().to_path_buf());
        project.model = Some(LoadedModel {
            descriptors: GeometryDescriptors::default(),
            bounds: None,
            form: sketch_form(),
        });

        let path = project.request_export(ExportFormat::Dxf).unwrap();

        assert_eq!(path, dir.path().join("bracket.dxf"));
        assert_eq!(project.exports_in_flight, 1);
        match cmd_rx.recv().unwrap() {
            OrchestratorCommand::Export {
                model,
                format,
                plane,
                ..
            } => {
                assert_eq!(model, ModelFile(PathBuf::from("/scratch/sketch-1.brep")));
                assert_eq!(format, ExportFormat::Dxf);
                assert_eq!(plane.map(|plane| plane.normal), Some([0.0, 1.0, 0.0]));
            }
            _ => panic!("sketch export must send an export command"),
        }

        let error = project.request_export(ExportFormat::Stl).unwrap_err();
        assert!(error.contains("STL"), "{error}");
        assert!(error.contains("a sketch"), "{error}");
        assert_eq!(project.exports_in_flight, 1);
    }

    #[test]
    fn an_unclosed_part_is_refused_before_any_export_command_is_sent() {
        let dir = tempfile::tempdir().unwrap();
        let (mut project, cmd_rx) = project_for_test(dir.path().to_path_buf());
        project.model_parts.push(part_for_export_test(
            ModelFile(PathBuf::from("/worker-scratch/bracket.brep")),
            vec![SolidValidity {
                valid: true,
                closed: false,
                ..SolidValidity::default()
            }],
        ));

        let error = project
            .request_part_export(7, ExportFormat::Stl)
            .unwrap_err();

        assert!(error.contains("Cannot export"), "unexpected error: {error}");
        assert_eq!(project.exports_in_flight, 0);
        assert!(cmd_rx.try_recv().is_err());
    }
}
