// A project folder as the application holds it open: the worker thread
// executing its script, its design history, its conversation on disk,
// and the model currently on screen. Everything here is replaced when
// another folder is opened; nothing here touches egui or the GPU.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use cadmark_bridge::AiServices;
use cadmark_core::cancellation::CancelFlag;
use cadmark_core::context::{IdentificationStrategy, MeasuredIdentification, NullIdentification};
use cadmark_core::export::ExportFormat;
use cadmark_core::geometry::{GeometryDescriptors, ModelSummary, SolidValidity, TopologyElement};
use cadmark_core::ledger::ProvenanceLedger;
use cadmark_core::limits::ExecutionLimits;
use cadmark_core::message::{Conversation, Message};
use cadmark_core::version::VersionHistory;
use cadmark_kernel::protocol::{ExecutedModel, ModelFile};
use cadmark_renderer::camera::Bounds3;

use crate::orchestrator::{OrchestratorCommand, OrchestratorResult, spawn_orchestrator};
use crate::turn::TurnInput;
use crate::validity::{ExportDecision, export_decision, export_warning};

/// The one script a project folder holds at present.
pub const SCRIPT_FILENAME: &str = "part.py";

/// Where the conversation is kept, inside the project folder.
const CONVERSATION_FILENAME: &str = ".cadmark/conversation.json";
const CONVERSATION_ARCHIVE_DIRECTORY: &str = ".cadmark/conversations";

/// How often the script on disk is compared with the model on screen.
pub const SCRIPT_WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// The model on screen: what the application keeps from the last
/// successful execution besides the mesh, which lives on the GPU.
pub struct LoadedModel {
    pub summary: ModelSummary,
    pub descriptors: GeometryDescriptors,
    pub bounds: Option<Bounds3>,
    pub model: ModelFile,
    /// Per-solid kernel validity retained for status and the export gate.
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
    /// Identification strategy for geometry context, rebuilt from the
    /// measured geometry of each executed model.
    pub identification: Box<dyn IdentificationStrategy>,
    /// The model on screen, if a script has executed successfully.
    pub model: Option<LoadedModel>,
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
    /// Open a folder: initialise its git repository, load its history and
    /// conversation, start a worker, and ask for the script to be built.
    pub fn open(dir: PathBuf, ai: Result<AiServices, String>, limits: ExecutionLimits) -> Self {
        let dir = dir.canonicalize().unwrap_or(dir);

        if let Err(e) = crate::git_ops::ensure_repo(&dir) {
            log::error!("Failed to initialise git repo in {}: {e}", dir.display());
        }

        let mut conversation = load_conversation(&dir);
        if conversation.is_empty() {
            conversation.push(Message::notice(
                "Describe what you'd like to build, or click a face, edge or vertex of the \
                 model to comment on it. Every completed turn is saved to part.py in the \
                 project folder and recorded as a design step.",
            ));
        }
        let ai_model = match &ai {
            Ok(services) => Some(services.model.model_name().to_string()),
            Err(reason) => {
                log::warn!("{reason}");
                conversation.push(Message::notice(format!(
                    "AI is unavailable: {reason}. The model still loads, and you can edit \
                     {SCRIPT_FILENAME} by hand and press Rebuild."
                )));
                None
            }
        };
        let (cmd_tx, result_rx) =
            spawn_orchestrator(dir.clone(), SCRIPT_FILENAME.to_string(), ai, limits);

        let history = match crate::git_ops::list_microversions(&dir, 100) {
            Ok(versions) => VersionHistory::from_versions(versions),
            Err(e) => {
                log::warn!("Failed to load microversion history: {e}");
                VersionHistory::new()
            }
        };

        let mut project = Self {
            dir,
            cmd_tx,
            result_rx,
            history,
            conversation,
            ai_model,
            busy: None,
            ledger: ProvenanceLedger::new(),
            identification: Box::new(NullIdentification),
            model: None,
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
        self.dir.join(SCRIPT_FILENAME)
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

    /// Write the current model next to the script.
    pub fn request_export(&mut self, format: ExportFormat) -> Result<PathBuf, String> {
        let model = self.model.as_ref().ok_or("no model to export")?;
        let decision = export_decision(&model.validity);
        if decision != ExportDecision::Ready {
            return Err(export_warning(&decision).expect("non-ready decision has warning"));
        }
        let path = self.dir.join(format!("part.{}", format.extension()));
        self.send(OrchestratorCommand::Export {
            model: model.model.clone(),
            format,
            path: path.clone(),
        })?;
        self.exports_in_flight += 1;
        Ok(path)
    }

    /// Ask the retained worker model for the closest separation of two picked elements.
    pub fn request_minimum_distance(
        &mut self,
        first: TopologyElement,
        second: TopologyElement,
    ) -> Result<(), String> {
        let model = self.model.as_ref().ok_or("no model to measure")?;
        self.send(OrchestratorCommand::MinimumDistance {
            model: model.model.clone(),
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
        let bounds = Bounds3::from_positions(model.mesh.vertices.iter().map(|v| v.position));
        self.ledger = model.ledger;
        self.identification = Box::new(MeasuredIdentification {
            descriptors: model.descriptors.clone(),
        });
        self.model = Some(LoadedModel {
            summary: model.summary,
            descriptors: model.descriptors,
            bounds,
            model: model.model,
            validity: model.validity,
        });
        self.script_source = Some(source);
        self.has_script = true;
        self.record_script_state();
        first_model.then_some(bounds).flatten()
    }

    /// Forget the model on screen so a failed reload shows nothing stale.
    pub fn clear_model(&mut self) {
        self.ledger.clear();
        self.identification = Box::new(NullIdentification);
        self.model = None;
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
        "Started a new conversation. The earlier conversation is saved in .cadmark/conversations; part.py is unchanged.",
    ));
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let script = dir.path().join(SCRIPT_FILENAME);
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
}
