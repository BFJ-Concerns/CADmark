// Orchestrator — coordinates the AI request → execute → render cycle.
//
// When the user sends a chat message or spatial comment, the orchestrator:
// 1. Packages context (current code, geometry, conversation) into an AiRequest.
// 2. Sends the request to the AI backend (async, off main thread).
// 3. Receives modified code.
// 4. Executes the build123d script via the kernel.
// 5. If execution fails, retries with the traceback (one attempt).
// 6. On success: updates the mesh, creates a microversion commit, updates chat.
// 7. On failure: reports the error in chat.

use std::path::PathBuf;
use std::sync::mpsc;

use cadmark_bridge::backend::{AiBackend, AiResponse};
use cadmark_bridge::context::AiRequest;
use cadmark_bridge::{AiServices, doc_lookup::DocLookup};
use cadmark_core::geometry::GeometryContext;
use cadmark_core::message::MessageId;

/// Commands sent from the UI thread to the orchestrator.
pub enum OrchestratorCommand {
    /// User typed a chat message.
    ChatMessage(String),
    /// User submitted a spatial comment with geometry context.
    SpatialComment {
        id: MessageId,
        text: String,
        context: GeometryContext,
    },
    /// Undo/redo changed the working tree — sync the cached source code.
    UpdateCode(String),
}

/// Results sent from the orchestrator back to the UI thread.
pub enum OrchestratorResult {
    /// AI responded successfully — code executed and mesh is ready.
    Success {
        response: AiResponse,
        trigger_message: String,
        applied_spatial_message_id: Option<MessageId>,
        /// Path to the updated script file (for the kernel to re-read).
        script_path: PathBuf,
    },
    /// AI responded but code execution failed after retry.
    ExecutionFailed { ai_message: String, error: String },
    /// AI backend itself failed.
    BackendError(String),
}

/// Maximum number of previous user messages retained for doc lookup context.
const MAX_MESSAGE_HISTORY: usize = 10;

/// Manages the async AI request cycle.
/// Lives on a background tokio runtime, communicates with the UI via channels.
pub struct Orchestrator {
    /// Current build123d source code.
    current_code: String,
    /// Path to the project directory.
    project_dir: PathBuf,
    /// Path to the script file within the project.
    script_filename: String,
    /// Rolling window of recent user messages, used by the doc lookup
    /// agent to understand conversational context.
    message_history: Vec<String>,
    backend: Box<dyn AiBackend>,
    doc_lookup: DocLookup,
    script_executor: Box<dyn ScriptExecutor>,
}

impl Orchestrator {
    fn new(
        project_dir: PathBuf,
        script_filename: String,
        services: AiServices,
        script_executor: Box<dyn ScriptExecutor>,
    ) -> Self {
        Self {
            current_code: String::new(),
            project_dir,
            script_filename,
            message_history: Vec::new(),
            backend: Box::new(services.model_edit),
            doc_lookup: services.doc_lookup,
            script_executor,
        }
    }

    pub fn script_path(&self) -> PathBuf {
        self.project_dir.join(&self.script_filename)
    }

    /// Load the current script from disk (for session resume).
    pub fn load_current_code(&mut self) -> Result<(), std::io::Error> {
        let path = self.script_path();
        if path.exists() {
            self.current_code = std::fs::read_to_string(&path)?;
        }
        Ok(())
    }

    /// Process a chat message through the AI.
    /// Runs the doc lookup agent first to gather relevant API references.
    pub async fn handle_chat(&mut self, message: &str) -> OrchestratorResult {
        let doc_context = self.doc_lookup.lookup(message, &self.message_history).await;
        self.push_message(message);

        let mut request = AiRequest::from_chat(self.current_code.clone(), message.to_string());
        if let Some(docs) = doc_context {
            request = request.with_doc_context(docs);
        }
        self.process_request(request, None).await
    }

    /// Process a spatial comment through the AI.
    /// Runs the doc lookup agent first to gather relevant API references.
    pub async fn handle_spatial_comment(
        &mut self,
        id: MessageId,
        text: &str,
        context: GeometryContext,
    ) -> OrchestratorResult {
        let doc_context = self.doc_lookup.lookup(text, &self.message_history).await;
        self.push_message(text);

        let mut request =
            AiRequest::from_spatial_comment(self.current_code.clone(), text.to_string(), context);
        if let Some(docs) = doc_context {
            request = request.with_doc_context(docs);
        }
        self.process_request(request, Some(id)).await
    }

    /// Core request processing — send to AI, execute, retry on failure.
    async fn process_request(
        &mut self,
        request: AiRequest,
        applied_spatial_message_id: Option<MessageId>,
    ) -> OrchestratorResult {
        let trigger_message = request.user_message.clone();
        // Step 1: Send to AI.
        let response = match self.backend.request(request.clone()).await {
            Ok(r) => r,
            Err(e) => return OrchestratorResult::BackendError(e.to_string()),
        };

        // Step 2: Write the new code and try executing it.
        let script_path = self.script_path();
        if let Err(e) = std::fs::write(&script_path, &response.code) {
            return OrchestratorResult::ExecutionFailed {
                ai_message: response.message.clone(),
                error: format!("Failed to write script: {e}"),
            };
        }

        match self.script_executor.execute(&script_path) {
            Ok(()) => {
                // Execution succeeded — update state.
                self.current_code = response.code.clone();
                OrchestratorResult::Success {
                    response,
                    trigger_message,
                    applied_spatial_message_id,
                    script_path,
                }
            }
            Err(first_error) => {
                // Step 3: Retry with traceback.
                let traceback = format!("{first_error}");
                let retry_request = request.retry_with_traceback(traceback.clone());

                match self.backend.request(retry_request).await {
                    Ok(retry_response) => {
                        // Write the retry code.
                        if let Err(e) = std::fs::write(&script_path, &retry_response.code) {
                            self.restore_original(&script_path);
                            return OrchestratorResult::ExecutionFailed {
                                ai_message: retry_response.message,
                                error: format!("Failed to write retry script: {e}"),
                            };
                        }

                        match self.script_executor.execute(&script_path) {
                            Ok(()) => {
                                self.current_code = retry_response.code.clone();
                                OrchestratorResult::Success {
                                    response: retry_response,
                                    trigger_message,
                                    applied_spatial_message_id,
                                    script_path,
                                }
                            }
                            Err(retry_error) => {
                                // Restore the original code on double failure.
                                self.restore_original(&script_path);
                                OrchestratorResult::ExecutionFailed {
                                    ai_message: retry_response.message,
                                    error: format!(
                                        "Code failed after retry.\n\nFirst error:\n{traceback}\n\nRetry error:\n{retry_error}"
                                    ),
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // Restore the original code.
                        self.restore_original(&script_path);
                        OrchestratorResult::BackendError(format!(
                            "Retry failed: {e}\n\nOriginal error:\n{traceback}"
                        ))
                    }
                }
            }
        }
    }

    /// Update the stored code (e.g. after undo/redo restores a different version).
    pub fn set_current_code(&mut self, code: String) {
        self.current_code = code;
    }

    pub fn current_code(&self) -> &str {
        &self.current_code
    }

    /// Record a user message for doc lookup context.
    /// Keeps only the most recent messages to avoid unbounded growth.
    fn push_message(&mut self, message: &str) {
        self.message_history.push(message.to_string());
        if self.message_history.len() > MAX_MESSAGE_HISTORY {
            self.message_history.remove(0);
        }
    }

    fn restore_original(&self, script_path: &std::path::Path) {
        if let Err(error) = std::fs::write(script_path, &self.current_code) {
            log::error!(
                "Failed to restore script after request failure: {error}. \
                 File may contain broken AI-generated code."
            );
        }
    }
}

trait ScriptExecutor: Send + Sync {
    fn execute(&self, script_path: &std::path::Path) -> Result<(), String>;
}

struct KernelScriptExecutor;

impl ScriptExecutor for KernelScriptExecutor {
    fn execute(&self, script_path: &std::path::Path) -> Result<(), String> {
        cadmark_kernel::execution::execute_script(script_path)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Spawn the orchestrator on a tokio runtime with channel communication.
/// Returns (sender, receiver) for the UI thread to use.
pub fn spawn_orchestrator(
    project_dir: PathBuf,
    script_filename: String,
    services: AiServices,
) -> (
    mpsc::Sender<OrchestratorCommand>,
    mpsc::Receiver<OrchestratorResult>,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<OrchestratorCommand>();
    let (result_tx, result_rx) = mpsc::channel::<OrchestratorResult>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            let mut orchestrator = Orchestrator::new(
                project_dir,
                script_filename,
                services,
                Box::new(KernelScriptExecutor),
            );
            if let Err(e) = orchestrator.load_current_code() {
                log::error!("Failed to load existing script: {e}");
                let _ = result_tx.send(OrchestratorResult::BackendError(format!(
                    "Could not read project script: {e}"
                )));
            }

            while let Ok(cmd) = cmd_rx.recv() {
                match cmd {
                    // Code sync from undo/redo — no result to send back.
                    OrchestratorCommand::UpdateCode(code) => {
                        orchestrator.set_current_code(code);
                        continue;
                    }
                    cmd => {
                        let result = match cmd {
                            OrchestratorCommand::ChatMessage(msg) => {
                                orchestrator.handle_chat(&msg).await
                            }
                            OrchestratorCommand::SpatialComment { id, text, context } => {
                                orchestrator
                                    .handle_spatial_comment(id, &text, context)
                                    .await
                            }
                            OrchestratorCommand::UpdateCode(_) => unreachable!(),
                        };

                        if result_tx.send(result).is_err() {
                            break; // UI thread dropped the receiver.
                        }
                    }
                }
            }
        });
    });

    (cmd_tx, result_rx)
}
