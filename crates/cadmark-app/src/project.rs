// A project folder as the application holds it open: the worker thread
// executing its script, its design history, its conversation on disk,
// and the model currently on screen. Everything here is replaced when
// another folder is opened; nothing here touches egui or the GPU.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use cadmark_bridge::AiServices;
use cadmark_bridge::backend::TurnModel;
use cadmark_core::cancellation::CancelFlag;
use cadmark_core::context::{IdentificationStrategy, MeasuredIdentification, NullIdentification};
use cadmark_core::export::ExportFormat;
use cadmark_core::geometry::{
    GeometryDescriptors, ModelSummary, PartMeasurements, SolidValidity, TopologyElement,
};
use cadmark_core::ledger::ProvenanceLedger;
use cadmark_core::limits::ExecutionLimits;
use cadmark_core::message::{Conversation, Message};
use cadmark_core::sketch::SketchProfile;
use cadmark_core::sketch_lineage::SketchLineageLedger;
use cadmark_core::version::VersionHistory;
use cadmark_kernel::protocol::{ExecutedModel, ModelFile, ModelForm, SolidResult};
use cadmark_renderer::camera::Bounds3;

use crate::orchestrator::{OrchestratorCommand, OrchestratorResult, spawn_orchestrator};
use crate::parts::{self, OpenPart};
use crate::reference_images::ReferenceImages;
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

impl LoadedPart {
    /// The part's measurements under the name the script gave it.
    pub fn measurements(&self) -> PartMeasurements {
        PartMeasurements::new(self.name.clone(), self.summary.clone())
    }
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
    pub reference_images: ReferenceImages,
    pub ai_accepts_images: bool,
    /// The part of the folder currently being modelled.
    part: OpenPart,
    /// Every part script in the folder, for the switcher.
    parts: Vec<String>,
    cmd_tx: mpsc::Sender<OrchestratorCommand>,
    result_rx: mpsc::Receiver<OrchestratorResult>,
    pub history: VersionHistory,
    pub conversation: Conversation,
    /// The AI model in use, for the toolbar badge; absent when AI is
    /// unavailable and the reason is in the conversation.
    pub ai_model: Option<String>,
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
        let (cmd_tx, result_rx) = spawn_orchestrator(
            dir.clone(),
            part.file_name().to_string(),
            ai,
            limits,
            render,
        );

        let history = load_history(&dir);

        let mut project = Self {
            reference_images: ReferenceImages::load(&dir),
            ai_accepts_images,
            parts: parts::list_parts(&dir),
            part,
            dir,
            cmd_tx,
            result_rx,
            history,
            conversation,
            ai_model,
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

    /// Hand a command to the worker.
    pub fn send(&mut self, command: OrchestratorCommand) -> Result<(), String> {
        self.cmd_tx
            .send(command)
            .map_err(|_| "The modelling worker has stopped; restart CADmark".to_string())
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
        mut input: TurnInput,
        history: Conversation,
    ) -> Result<CancelFlag, String> {
        input.images = if self.ai_accepts_images {
            self.reference_images.inputs()
        } else {
            Vec::new()
        };
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

    /// Write the current model next to the script.
    pub fn request_export(&mut self, format: ExportFormat) -> Result<PathBuf, String> {
        let model = self.model.as_ref().ok_or("no model to export")?;
        let solid = model
            .solid()
            .ok_or("a sketch cannot be exported until the script makes it a solid")?;
        let decision = export_decision(&solid.validity);
        if decision != ExportDecision::Ready {
            return Err(export_warning(&decision).expect("non-ready decision has warning"));
        }
        let stem = parts::part_display_name(self.part.file_name());
        let path = self.dir.join(format!("{stem}.{}", format.extension()));
        self.send(OrchestratorCommand::Export {
            model: solid.file.clone(),
            format,
            path: path.clone(),
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
        })?;
        self.exports_in_flight += 1;
        Ok(path)
    }

    /// Every part on screen measured, in source binding order; empty when
    /// nothing solid is loaded. What a turn's change report compares against.
    pub fn part_measurements(&self) -> Vec<PartMeasurements> {
        self.model_parts
            .iter()
            .map(LoadedPart::measurements)
            .collect()
    }

    /// Make one part's local topology IDs the active selection domain after
    /// it has been picked as a whole.
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

    /// Ask the retained worker model for the closest separation of two picked elements.
    pub fn request_minimum_distance(
        &mut self,
        first: TopologyElement,
        second: TopologyElement,
    ) -> Result<(), String> {
        let model = self.model.as_ref().ok_or("no model to measure")?;
        let solid = model
            .solid()
            .ok_or("a sketch has no solid to measure between")?;
        self.send(OrchestratorCommand::MinimumDistance {
            model: solid.file.clone(),
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
        while let Ok(result) = self.result_rx.try_recv() {
            results.push(result);
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
            ModelForm::Sketch(sketch) => Bounds3::from_positions(sketch.points()),
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

    /// Persist the conversation beside the script.
    pub fn save_conversation(&self) {
        let path = self.dir.join(CONVERSATION_FILENAME);
        let write = || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let contents =
                serde_json::to_string(&self.conversation).expect("conversation serialises");
            std::fs::write(&path, contents)
        };
        if let Err(error) = write() {
            log::warn!("Could not save the conversation: {error}");
        }
    }

    /// Archive the conversation before beginning a blank one. The script is
    /// intentionally untouched: it remains the project's source of truth.
    pub fn start_fresh_conversation(&mut self) -> Result<(), String> {
        fresh_conversation(&self.dir, &mut self.conversation)?;
        self.save_conversation();
        Ok(())
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

fn load_conversation(dir: &Path) -> Conversation {
    match std::fs::read_to_string(dir.join(CONVERSATION_FILENAME)) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|error| {
            log::warn!("Ignoring unreadable conversation: {error}");
            Conversation::new()
        }),
        Err(_) => Conversation::new(),
    }
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
        ModelForm::Sketch(SketchProfile {
            plane: cadmark_core::sketch::SketchPlane {
                origin: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
                x_axis: [1.0, 0.0, 0.0],
            },
            curves: Vec::new(),
            corners: Vec::new(),
            regions: Vec::new(),
        })
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
                summary: ModelSummary {
                    volume: 1000.0,
                    bounds_min: [0.0; 3],
                    bounds_max: [10.0; 3],
                    face_count: 6,
                    edge_count: 12,
                    vertex_count: 8,
                },
                validity: vec![SolidValidity {
                    closed: true,
                    valid: true,
                }],
                file: ModelFile(PathBuf::from("/scratch/model-1.brep")),
                parts: Vec::new(),
            }),
        };

        assert!(sketch.solid().is_none());
        assert!(sketch.sketch().is_some());
        assert!(sketch.summary().is_none());
        assert!(sketch.validity().is_empty());

        assert!(solid.sketch().is_none());
        assert_eq!(solid.summary().map(|summary| summary.face_count), Some(6));
        assert_eq!(solid.validity().len(), 1);
    }

    /// A project with no worker behind it: the receiver stands in for the
    /// orchestrator so a command can be read back off the channel.
    fn project_for_test(dir: PathBuf) -> (Project, mpsc::Receiver<OrchestratorCommand>) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (_result_tx, result_rx) = mpsc::channel();
        let project = Project {
            reference_images: ReferenceImages::load(&dir),
            ai_accepts_images: false,
            parts: Vec::new(),
            part: OpenPart::Named("bracket.py".to_string()),
            dir,
            cmd_tx,
            result_rx,
            history: VersionHistory::new(),
            conversation: Conversation::new(),
            ai_model: None,
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
        };
        (project, cmd_rx)
    }

    #[test]
    fn project_references_survive_fresh_conversations_and_reopening_and_feed_each_turn() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("drawing.png");
        image::RgbImage::from_pixel(48, 24, image::Rgb([20, 80, 160]))
            .save(&source)
            .unwrap();
        let bytes = std::fs::read(&source).unwrap();
        crate::reference_images::attach(dir.path(), &source).unwrap();
        source_dir.close().unwrap();
        let (mut project, commands) = project_for_test(dir.path().to_path_buf());
        project.ai_accepts_images = true;
        for _ in 0..2 {
            project
                .conversation
                .push(Message::user_chat("old-session-marker"));
            project.start_fresh_conversation().unwrap();
            project
                .start_turn(TurnInput::default(), project.conversation.clone())
                .unwrap();
            let OrchestratorCommand::Turn {
                input,
                conversation,
                ..
            } = commands.recv().unwrap()
            else {
                panic!("expected a turn");
            };
            assert!(
                !serde_json::to_string(&conversation)
                    .unwrap()
                    .contains("old-session-marker")
            );
            assert_eq!(input.images.len(), 1);
            assert_eq!(input.images[0].bytes, bytes);
        }
        drop(project);
        let (mut reopened, commands) = project_for_test(dir.path().to_path_buf());
        reopened.ai_accepts_images = true;
        reopened
            .start_turn(TurnInput::default(), Conversation::new())
            .unwrap();
        let OrchestratorCommand::Turn { input, .. } = commands.recv().unwrap() else {
            panic!("expected a turn");
        };
        assert_eq!(input.images[0].bytes, bytes);
        reopened.ai_accepts_images = false;
        reopened
            .start_turn(TurnInput::default(), Conversation::new())
            .unwrap();
        let OrchestratorCommand::Turn { input, .. } = commands.recv().unwrap() else {
            panic!("expected a turn");
        };
        assert!(input.images.is_empty());
        assert_eq!(reopened.reference_images.images.len(), 1);
    }

    fn part_for_export_test(model: ModelFile, validity: Vec<SolidValidity>) -> LoadedPart {
        LoadedPart {
            id: 7,
            name: "bracket".to_string(),
            summary: ModelSummary {
                volume: 1.0,
                bounds_min: [0.0; 3],
                bounds_max: [1.0; 3],
                face_count: 1,
                edge_count: 1,
                vertex_count: 1,
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
            }],
        ));

        let path = project.request_part_export(7, ExportFormat::Stl).unwrap();

        assert_eq!(project.exports_in_flight, 1);
        match cmd_rx.recv().unwrap() {
            OrchestratorCommand::Export {
                model,
                format,
                path: command_path,
            } => {
                assert_eq!(model, kept_brep);
                assert_eq!(format, ExportFormat::Stl);
                assert_eq!(command_path, path);
            }
            _ => panic!("part export must send an export command"),
        }
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
