// The modelling worker thread: owns the kernel worker process and the AI
// model, and runs one command at a time — a reload of the script on disk,
// an AI turn, an export — reporting progress and results back to the UI
// thread over a channel. Nothing here touches egui or the GPU.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use cadmark_bridge::AiServices;
use cadmark_core::cancellation::CancelFlag;
use cadmark_core::export::ExportFormat;
use cadmark_core::limits::ExecutionLimits;
use cadmark_core::message::Conversation;
use cadmark_kernel::protocol::{ExecutedModel, ModelFile};
use cadmark_kernel::worker::{KernelWorker, WorkerError, WorkerLaunch};

use crate::turn::{
    DocSource, RenderSource, ScriptExecutor, TurnEvent, TurnInput, TurnOutcome, TurnRunner,
};

/// Commands sent from the UI thread to the worker.
pub enum OrchestratorCommand {
    /// Re-read the script from disk and execute it: startup, undo/redo, a
    /// parameter edit, and the Rebuild button.
    Reload,
    /// Run one AI turn. `conversation` is the history the model is shown;
    /// `cancel` ends the turn early.
    Turn {
        input: TurnInput,
        conversation: Conversation,
        cancel: CancelFlag,
    },
    /// Write a kept model to `path`.
    Export {
        model: ModelFile,
        format: ExportFormat,
        path: PathBuf,
    },
    /// The user changed the execution ceilings in settings.
    SetLimits(ExecutionLimits),
}

/// Results and progress sent from the worker back to the UI thread.
pub enum OrchestratorResult {
    /// The script on disk executed; show its model.
    Reloaded {
        model: Box<ExecutedModel>,
        source: String,
    },
    /// There is no script on disk yet: the normal state of a new project.
    NoScript,
    /// The script on disk failed to execute.
    ReloadFailed { error: String },
    /// Something happened during the running turn.
    TurnEvent(TurnEvent),
    /// The turn ended.
    TurnEnded(TurnOutcome),
    /// The export finished.
    Exported {
        format: ExportFormat,
        result: Result<PathBuf, String>,
    },
}

/// The production executor: the confined kernel worker.
struct WorkerExecutor {
    worker: KernelWorker,
    limits: ExecutionLimits,
}

impl ScriptExecutor for WorkerExecutor {
    fn execute(
        &mut self,
        script_path: &Path,
        cancel: &CancelFlag,
    ) -> Result<ExecutedModel, WorkerError> {
        self.worker.execute(script_path, self.limits, cancel)
    }
}

impl DocSource for cadmark_bridge::doc_lookup::DocLookup {
    fn lookup(
        &self,
        query: &str,
        cancel: CancelFlag,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + '_>> {
        let query = query.to_string();
        Box::pin(async move { self.lookup(&query, cancel).await })
    }
}

/// The worker's state between commands.
struct Orchestrator {
    project_dir: PathBuf,
    script_filename: String,
    executor: WorkerExecutor,
    /// The AI services, absent when no usable configuration was found. The
    /// model still loads and renders without them.
    ai: Result<AiServices, String>,
    /// What answers the render tool. The viewport's offscreen renderer in
    /// production; `NoRender` where the application has no GPU to render
    /// with.
    render: Box<dyn RenderSource>,
}

impl Orchestrator {
    fn script_path(&self) -> PathBuf {
        self.project_dir.join(&self.script_filename)
    }

    fn handle_reload(&mut self) -> OrchestratorResult {
        let script_path = self.script_path();
        if !script_path.exists() {
            return OrchestratorResult::NoScript;
        }
        let source = match std::fs::read_to_string(&script_path) {
            Ok(source) => source,
            Err(error) => {
                return OrchestratorResult::ReloadFailed {
                    error: format!("Failed to read {}: {error}", script_path.display()),
                };
            }
        };
        match self.executor.execute(&script_path, &CancelFlag::new()) {
            Ok(model) => OrchestratorResult::Reloaded {
                model: Box::new(model),
                source,
            },
            Err(error) => OrchestratorResult::ReloadFailed {
                error: error.to_string(),
            },
        }
    }

    async fn handle_turn(
        &mut self,
        input: TurnInput,
        conversation: Conversation,
        cancel: CancelFlag,
        report: &mpsc::Sender<OrchestratorResult>,
    ) -> TurnOutcome {
        let ai = match &self.ai {
            Ok(ai) => ai,
            Err(reason) => {
                return TurnOutcome::Failed {
                    error: format!("AI is unavailable: {reason}"),
                };
            }
        };
        let script_path = self.script_path();
        let mut runner = TurnRunner {
            model: &ai.model,
            executor: &mut self.executor,
            docs: &ai.doc_lookup,
            render: self.render.as_mut(),
            script_path,
            cancel,
        };
        runner
            .run(&conversation, &input, |event| {
                let _ = report.send(OrchestratorResult::TurnEvent(event));
            })
            .await
    }

    fn handle_export(
        &mut self,
        model: &ModelFile,
        format: ExportFormat,
        path: &Path,
    ) -> OrchestratorResult {
        let result = self
            .executor
            .worker
            .export(model, format, path, self.executor.limits)
            .map(|()| path.to_path_buf())
            .map_err(|error| error.to_string());
        OrchestratorResult::Exported { format, result }
    }
}

/// Spawn the worker on its own thread with channel communication.
/// Returns (sender, receiver) for the UI thread to use.
///
/// `ai` is the AI configuration outcome; when it is an error the worker
/// still executes scripts and answers a turn with the reason. `render`
/// answers the render tool from the UI thread's published scene.
pub fn spawn_orchestrator(
    project_dir: PathBuf,
    script_filename: String,
    ai: Result<AiServices, String>,
    limits: ExecutionLimits,
    render: Box<dyn RenderSource>,
) -> (
    mpsc::Sender<OrchestratorCommand>,
    mpsc::Receiver<OrchestratorResult>,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<OrchestratorCommand>();
    let (result_tx, result_rx) = mpsc::channel();

    std::thread::Builder::new()
        .name("cadmark-orchestrator".into())
        .spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
            rt.block_on(async {
                let launch = match WorkerLaunch::beside_current_exe(project_dir.clone()) {
                    Ok(launch) => launch,
                    Err(error) => {
                        let _ = result_tx.send(OrchestratorResult::ReloadFailed {
                            error: error.to_string(),
                        });
                        return;
                    }
                };
                let mut orchestrator = Orchestrator {
                    project_dir,
                    script_filename,
                    executor: WorkerExecutor {
                        worker: KernelWorker::new(launch),
                        limits,
                    },
                    ai,
                    render,
                };

                while let Ok(command) = cmd_rx.recv() {
                    let result = match command {
                        OrchestratorCommand::Reload => orchestrator.handle_reload(),
                        OrchestratorCommand::Turn {
                            input,
                            conversation,
                            cancel,
                        } => OrchestratorResult::TurnEnded(
                            orchestrator
                                .handle_turn(input, conversation, cancel, &result_tx)
                                .await,
                        ),
                        OrchestratorCommand::Export {
                            model,
                            format,
                            path,
                        } => orchestrator.handle_export(&model, format, &path),
                        OrchestratorCommand::SetLimits(limits) => {
                            orchestrator.executor.limits = limits;
                            continue;
                        }
                    };
                    if result_tx.send(result).is_err() {
                        break; // UI thread dropped the receiver.
                    }
                }
            });
        })
        .expect("failed to spawn the orchestrator thread");

    (cmd_tx, result_rx)
}
