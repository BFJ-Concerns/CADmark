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
use cadmark_core::geometry::GeometryContext;

/// Commands sent from the UI thread to the orchestrator.
pub enum OrchestratorCommand {
    /// User typed a chat message.
    ChatMessage(String),
    /// User submitted a spatial comment with geometry context.
    SpatialComment {
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
        /// Path to the updated script file (for the kernel to re-read).
        script_path: PathBuf,
    },
    /// AI responded but code execution failed after retry.
    ExecutionFailed {
        ai_message: String,
        error: String,
    },
    /// AI backend itself failed.
    BackendError(String),
}

/// Manages the async AI request cycle.
/// Lives on a background tokio runtime, communicates with the UI via channels.
pub struct Orchestrator {
    /// Current build123d source code.
    current_code: String,
    /// Path to the project directory.
    project_dir: PathBuf,
    /// Path to the script file within the project.
    script_filename: String,
}

impl Orchestrator {
    pub fn new(project_dir: PathBuf, script_filename: String) -> Self {
        Self {
            current_code: String::new(),
            project_dir,
            script_filename,
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
    pub async fn handle_chat(
        &mut self,
        message: &str,
        backend: &dyn AiBackend,
    ) -> OrchestratorResult {
        let request = AiRequest::from_chat(self.current_code.clone(), message.to_string());
        self.process_request(request, backend).await
    }

    /// Process a spatial comment through the AI.
    pub async fn handle_spatial_comment(
        &mut self,
        text: &str,
        context: GeometryContext,
        backend: &dyn AiBackend,
    ) -> OrchestratorResult {
        let request = AiRequest::from_spatial_comment(
            self.current_code.clone(),
            text.to_string(),
            context,
        );
        self.process_request(request, backend).await
    }

    /// Core request processing — send to AI, execute, retry on failure.
    async fn process_request(
        &mut self,
        request: AiRequest,
        backend: &dyn AiBackend,
    ) -> OrchestratorResult {
        // Step 1: Send to AI.
        let response = match backend.request(request.clone()).await {
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

        match cadmark_kernel::execution::execute_script(&script_path) {
            Ok(_result) => {
                // Execution succeeded — update state.
                self.current_code = response.code.clone();
                OrchestratorResult::Success {
                    response,
                    script_path,
                }
            }
            Err(first_error) => {
                // Step 3: Retry with traceback.
                let traceback = format!("{first_error}");
                let retry_request = request.retry_with_traceback(traceback.clone());

                match backend.request(retry_request).await {
                    Ok(retry_response) => {
                        // Write the retry code.
                        if let Err(e) = std::fs::write(&script_path, &retry_response.code) {
                            return OrchestratorResult::ExecutionFailed {
                                ai_message: retry_response.message,
                                error: format!("Failed to write retry script: {e}"),
                            };
                        }

                        match cadmark_kernel::execution::execute_script(&script_path) {
                            Ok(_result) => {
                                self.current_code = retry_response.code.clone();
                                OrchestratorResult::Success {
                                    response: retry_response,
                                    script_path,
                                }
                            }
                            Err(retry_error) => {
                                // Restore the original code on double failure.
                                if let Err(restore_err) = std::fs::write(&script_path, &self.current_code) {
                                    log::error!(
                                        "Failed to restore script after execution error: {restore_err}. \
                                         File may contain broken AI-generated code."
                                    );
                                }
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
                        if let Err(restore_err) = std::fs::write(&script_path, &self.current_code) {
                            log::error!(
                                "Failed to restore script after backend error: {restore_err}. \
                                 File may contain broken AI-generated code."
                            );
                        }
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
}

/// Spawn the orchestrator on a tokio runtime with channel communication.
/// Returns (sender, receiver) for the UI thread to use.
pub fn spawn_orchestrator(
    project_dir: PathBuf,
    script_filename: String,
) -> (
    mpsc::Sender<OrchestratorCommand>,
    mpsc::Receiver<OrchestratorResult>,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<OrchestratorCommand>();
    let (result_tx, result_rx) = mpsc::channel::<OrchestratorResult>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            let mut orchestrator = Orchestrator::new(project_dir.clone(), script_filename);
            if let Err(e) = orchestrator.load_current_code() {
                log::error!("Failed to load existing script: {e}");
                let _ = result_tx.send(OrchestratorResult::BackendError(
                    format!("Could not read project script: {e}"),
                ));
            }

            let backend =
                cadmark_bridge::claude_code::ClaudeCodeBackend::new(project_dir);

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
                                orchestrator.handle_chat(&msg, &backend).await
                            }
                            OrchestratorCommand::SpatialComment { text, context } => {
                                orchestrator
                                    .handle_spatial_comment(&text, context, &backend)
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
