// Orchestrator — the worker thread that owns script execution.
//
// Every build123d execution runs here, never on the UI thread: the initial
// load, reloads after undo/redo/refresh, and the AI edit cycle. For an AI
// request the orchestrator:
// 1. Packages context (current code, geometry, conversation) into an AiRequest.
// 2. Sends the request to the AI backend.
// 3. Writes the returned code and executes it once.
// 4. If execution fails for a reason the AI can fix, retries with the
//    traceback (one attempt).
// 5. Ships the executed model to the UI, or restores the previous script and
//    reports the failure.

use std::path::{Path, PathBuf};
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
    /// Re-read the script from disk and execute it: startup, undo/redo, and
    /// the Refresh button.
    Reload,
}

/// Why a model was built: a reload of what is on disk, or an accepted AI edit.
pub enum ModelOrigin {
    Reload,
    Edit(EditOutcome),
}

/// The AI edit that produced a model.
pub struct EditOutcome {
    pub response: AiResponse,
    pub trigger_message: String,
    pub applied_spatial_message_id: Option<MessageId>,
    /// Path to the script the edit was written to.
    pub script_path: PathBuf,
}

/// Results sent from the orchestrator back to the UI thread.
pub enum OrchestratorResult<M> {
    /// A script executed successfully and its model is ready to display.
    ModelReady { model: M, origin: ModelOrigin },
    /// AI responded but its code failed; the previous script was restored.
    ExecutionFailed { ai_message: String, error: String },
    /// There is no script on disk yet: the normal state of a new project.
    NoScript,
    /// The script on disk failed to execute.
    ReloadFailed { error: String },
    /// AI backend itself failed, or no AI is configured.
    BackendError(String),
}

/// Why a script execution failed, and whether handing the message back to
/// the AI could plausibly fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionFailure {
    pub message: String,
    /// True for faults in the script itself (a Python error, an unsupported
    /// output shape). False for faults in CADmark's own runtime, which the
    /// AI cannot repair and should not be told to.
    pub retryable: bool,
}

/// Executes a script and produces a model. The kernel is the production
/// implementation; tests substitute scripted outcomes.
pub trait ScriptExecutor: Send + Sync {
    type Model: Send + 'static;

    fn execute(&self, script_path: &Path) -> Result<Self::Model, ExecutionFailure>;
}

/// Maximum number of previous user messages retained for doc lookup context.
const MAX_MESSAGE_HISTORY: usize = 10;

/// Manages script execution and the AI request cycle on a worker thread.
pub struct Orchestrator<E: ScriptExecutor> {
    /// Current build123d source code.
    current_code: String,
    /// Path to the project directory.
    project_dir: PathBuf,
    /// Path to the script file within the project.
    script_filename: String,
    /// Rolling window of recent user messages, used by the doc lookup
    /// agent to understand conversational context.
    message_history: Vec<String>,
    /// AI consumers, absent when no usable configuration was found. The
    /// model still loads and renders without them.
    ai: Option<AiConsumers>,
    /// Why the AI consumers are absent, shown when the user tries to chat.
    ai_unavailable_reason: Option<String>,
    script_executor: E,
}

struct AiConsumers {
    backend: Box<dyn AiBackend>,
    doc_lookup: DocLookup,
}

impl<E: ScriptExecutor> Orchestrator<E> {
    fn new(
        project_dir: PathBuf,
        script_filename: String,
        services: Result<AiServices, String>,
        script_executor: E,
    ) -> Self {
        let (ai, ai_unavailable_reason) = match services {
            Ok(services) => (
                Some(AiConsumers {
                    backend: Box::new(services.model_edit),
                    doc_lookup: services.doc_lookup,
                }),
                None,
            ),
            Err(reason) => (None, Some(reason)),
        };
        Self {
            current_code: String::new(),
            project_dir,
            script_filename,
            message_history: Vec::new(),
            ai,
            ai_unavailable_reason,
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

    /// Re-read the script from disk and execute it.
    pub fn handle_reload(&mut self) -> OrchestratorResult<E::Model> {
        let script_path = self.script_path();
        if !script_path.exists() {
            self.current_code.clear();
            return OrchestratorResult::NoScript;
        }
        match std::fs::read_to_string(&script_path) {
            Ok(source) => self.current_code = source,
            Err(error) => {
                return OrchestratorResult::ReloadFailed {
                    error: format!("Failed to read {}: {error}", script_path.display()),
                };
            }
        }
        match self.script_executor.execute(&script_path) {
            Ok(model) => OrchestratorResult::ModelReady {
                model,
                origin: ModelOrigin::Reload,
            },
            Err(failure) => OrchestratorResult::ReloadFailed {
                error: failure.message,
            },
        }
    }

    /// Process a chat message through the AI.
    /// Runs the doc lookup agent first to gather relevant API references.
    pub async fn handle_chat(&mut self, message: &str) -> OrchestratorResult<E::Model> {
        let Some(ai) = &self.ai else {
            return self.ai_unavailable();
        };
        let doc_context = ai.doc_lookup.lookup(message, &self.message_history).await;
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
    ) -> OrchestratorResult<E::Model> {
        let Some(ai) = &self.ai else {
            return self.ai_unavailable();
        };
        let doc_context = ai.doc_lookup.lookup(text, &self.message_history).await;
        self.push_message(text);

        let mut request =
            AiRequest::from_spatial_comment(self.current_code.clone(), text.to_string(), context);
        if let Some(docs) = doc_context {
            request = request.with_doc_context(docs);
        }
        self.process_request(request, Some(id)).await
    }

    fn ai_unavailable(&self) -> OrchestratorResult<E::Model> {
        OrchestratorResult::BackendError(format!(
            "AI is unavailable: {}",
            self.ai_unavailable_reason
                .as_deref()
                .unwrap_or("no AI is configured")
        ))
    }

    /// Core request processing — send to AI, execute, retry on failure.
    async fn process_request(
        &mut self,
        request: AiRequest,
        applied_spatial_message_id: Option<MessageId>,
    ) -> OrchestratorResult<E::Model> {
        let Some(ai) = &self.ai else {
            return self.ai_unavailable();
        };
        let trigger_message = request.user_message.clone();
        // Step 1: Send to AI.
        let response = match ai.backend.request(request.clone()).await {
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

        let first_failure = match self.script_executor.execute(&script_path) {
            Ok(model) => {
                self.current_code = response.code.clone();
                return OrchestratorResult::ModelReady {
                    model,
                    origin: ModelOrigin::Edit(EditOutcome {
                        response,
                        trigger_message,
                        applied_spatial_message_id,
                        script_path,
                    }),
                };
            }
            Err(failure) => failure,
        };

        if !first_failure.retryable {
            // A fault in CADmark's own runtime: the AI cannot fix it, and
            // telling it to would only make it "repair" correct code.
            self.restore_original(&script_path);
            return OrchestratorResult::ExecutionFailed {
                ai_message: response.message,
                error: first_failure.message,
            };
        }

        // Step 3: Retry with traceback.
        let traceback = first_failure.message;
        let retry_request = request.retry_with_traceback(traceback.clone());

        let retry_response = match ai.backend.request(retry_request).await {
            Ok(retry_response) => retry_response,
            Err(e) => {
                self.restore_original(&script_path);
                return OrchestratorResult::BackendError(format!(
                    "Retry failed: {e}\n\nOriginal error:\n{traceback}"
                ));
            }
        };

        if let Err(e) = std::fs::write(&script_path, &retry_response.code) {
            self.restore_original(&script_path);
            return OrchestratorResult::ExecutionFailed {
                ai_message: retry_response.message,
                error: format!("Failed to write retry script: {e}"),
            };
        }

        match self.script_executor.execute(&script_path) {
            Ok(model) => {
                self.current_code = retry_response.code.clone();
                OrchestratorResult::ModelReady {
                    model,
                    origin: ModelOrigin::Edit(EditOutcome {
                        response: retry_response,
                        trigger_message,
                        applied_spatial_message_id,
                        script_path,
                    }),
                }
            }
            Err(retry_failure) => {
                // Restore the original code on double failure.
                self.restore_original(&script_path);
                OrchestratorResult::ExecutionFailed {
                    ai_message: retry_response.message,
                    error: format!(
                        "Code failed after retry.\n\nFirst error:\n{traceback}\n\nRetry error:\n{}",
                        retry_failure.message
                    ),
                }
            }
        }
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

    fn restore_original(&self, script_path: &Path) {
        if let Err(error) = std::fs::write(script_path, &self.current_code) {
            log::error!(
                "Failed to restore script after request failure: {error}. \
                 File may contain broken AI-generated code."
            );
        }
    }
}

/// The production executor: the embedded build123d kernel.
pub struct KernelScriptExecutor;

impl ScriptExecutor for KernelScriptExecutor {
    type Model = cadmark_kernel::execution::ExecutionResult;

    fn execute(&self, script_path: &Path) -> Result<Self::Model, ExecutionFailure> {
        use cadmark_kernel::execution::ExecutionError;
        cadmark_kernel::execution::execute_script(script_path).map_err(|error| {
            let retryable = matches!(
                error,
                ExecutionError::Script(_)
                    | ExecutionError::Tessellation(_)
                    | ExecutionError::NoSolid
            );
            ExecutionFailure {
                message: error.to_string(),
                retryable,
            }
        })
    }
}

/// Spawn the orchestrator on its own thread with channel communication.
/// Returns (sender, receiver) for the UI thread to use.
///
/// `services` is the AI configuration outcome; when it is an error the
/// orchestrator still executes scripts and answers chat with the reason.
pub fn spawn_orchestrator(
    project_dir: PathBuf,
    script_filename: String,
    services: Result<AiServices, String>,
) -> (
    mpsc::Sender<OrchestratorCommand>,
    mpsc::Receiver<OrchestratorResult<cadmark_kernel::execution::ExecutionResult>>,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<OrchestratorCommand>();
    let (result_tx, result_rx) = mpsc::channel();

    std::thread::Builder::new()
        .name("cadmark-orchestrator".into())
        .spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
            rt.block_on(async {
                let mut orchestrator =
                    Orchestrator::new(project_dir, script_filename, services, KernelScriptExecutor);
                if let Err(e) = orchestrator.load_current_code() {
                    log::error!("Failed to load existing script: {e}");
                    let _ = result_tx.send(OrchestratorResult::ReloadFailed {
                        error: format!("Could not read project script: {e}"),
                    });
                }

                while let Ok(cmd) = cmd_rx.recv() {
                    let result = match cmd {
                        OrchestratorCommand::Reload => orchestrator.handle_reload(),
                        OrchestratorCommand::ChatMessage(msg) => {
                            orchestrator.handle_chat(&msg).await
                        }
                        OrchestratorCommand::SpatialComment { id, text, context } => {
                            orchestrator
                                .handle_spatial_comment(id, &text, context)
                                .await
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use cadmark_bridge::config::AiConfiguration;
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const FAKE_CREDENTIAL: &str = "fake-recording-credential";

    #[derive(Debug)]
    struct RecordedRequest {
        path: String,
        authenticated: bool,
        body: Value,
    }

    impl RecordedRequest {
        /// The user text carried by the Responses message-array `input`.
        fn input_text(&self) -> &str {
            self.body["input"][0]["content"][0]["text"]
                .as_str()
                .expect("recorded request carries message-array input")
        }
    }

    struct ScriptedResponse {
        status: u16,
        body: String,
    }

    async fn recording_server(
        responses: Vec<ScriptedResponse>,
    ) -> (
        String,
        Arc<Mutex<Vec<RecordedRequest>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let records = Arc::new(Mutex::new(Vec::new()));
        let server_records = Arc::clone(&records);
        let handle = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let record = read_request(&mut stream).await;
                server_records.lock().unwrap().push(record);
                let reason = if response.status == 200 {
                    "OK"
                } else {
                    "Error"
                };
                let wire_response = format!(
                    "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    response.body.len(),
                    response.body
                );
                stream.write_all(wire_response.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{address}/v1"), records, handle)
    }

    async fn read_request(stream: &mut tokio::net::TcpStream) -> RecordedRequest {
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut chunk = [0_u8; 4096];
            let read = stream.read(&mut chunk).await.unwrap();
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
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0);
            bytes.extend_from_slice(&chunk[..read]);
        }

        let path = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_string();
        let authenticated = headers.lines().any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.eq_ignore_ascii_case("authorization")
                    && value
                        .trim()
                        .strip_prefix("Bearer ")
                        .is_some_and(|token| !token.is_empty())
            })
        });
        let body = serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap();
        RecordedRequest {
            path,
            authenticated,
            body,
        }
    }

    fn completed(text: &str) -> ScriptedResponse {
        ScriptedResponse {
            status: 200,
            body: serde_json::json!({
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": text}]
                }]
            })
            .to_string(),
        }
    }

    fn provider_failure(message: &str) -> ScriptedResponse {
        ScriptedResponse {
            status: 500,
            body: serde_json::json!({
                "error": {
                    "type": "provider_error",
                    "code": "scripted_failure",
                    "message": message
                }
            })
            .to_string(),
        }
    }

    fn services(base_url: &str) -> AiServices {
        let configuration = AiConfiguration::parse(&format!(
            r#"{{
                "ai": {{
                    "provider": "openai-compatible",
                    "base_url": "{base_url}",
                    "model": "recording-model",
                    "credential_env": "CADMARK_RECORDING_KEY",
                    "timeout_seconds": 5,
                    "allow_insecure_http": true
                }}
            }}"#
        ))
        .unwrap();
        cadmark_bridge::build_ai_services_with_env(configuration, |name| {
            (name == "CADMARK_RECORDING_KEY").then(|| FAKE_CREDENTIAL.to_string())
        })
        .unwrap()
    }

    #[derive(Clone)]
    struct FakeExecutor {
        state: Arc<Mutex<FakeExecutorState>>,
    }

    struct FakeExecutorState {
        results: VecDeque<Result<(), ExecutionFailure>>,
        executed_code: Vec<String>,
    }

    /// A script-level failure the AI may be asked to repair.
    fn script_failure(message: &str) -> ExecutionFailure {
        ExecutionFailure {
            message: message.to_string(),
            retryable: true,
        }
    }

    impl FakeExecutor {
        fn new(results: impl IntoIterator<Item = Result<(), String>>) -> Self {
            Self::scripted(
                results
                    .into_iter()
                    .map(|result| result.map_err(|message| script_failure(&message))),
            )
        }

        fn scripted(results: impl IntoIterator<Item = Result<(), ExecutionFailure>>) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeExecutorState {
                    results: results.into_iter().collect(),
                    executed_code: Vec::new(),
                })),
            }
        }

        fn executed_code(&self) -> Vec<String> {
            self.state.lock().unwrap().executed_code.clone()
        }
    }

    impl ScriptExecutor for FakeExecutor {
        type Model = String;

        fn execute(&self, script_path: &std::path::Path) -> Result<String, ExecutionFailure> {
            let code = std::fs::read_to_string(script_path).unwrap();
            let mut state = self.state.lock().unwrap();
            state.executed_code.push(code.clone());
            state.results.pop_front().unwrap_or(Ok(())).map(|()| code)
        }
    }

    fn orchestrator(
        project_dir: &std::path::Path,
        services: AiServices,
        executor: FakeExecutor,
    ) -> Orchestrator<FakeExecutor> {
        let mut orchestrator = Orchestrator::new(
            project_dir.to_path_buf(),
            "part.py".to_string(),
            Ok(services),
            executor,
        );
        orchestrator.load_current_code().unwrap();
        orchestrator
    }

    #[test]
    fn reload_executes_the_script_on_disk_and_reports_its_model() {
        let project = tempfile::tempdir().unwrap();
        let script_path = project.path().join("part.py");
        let executor = FakeExecutor::new([Ok(())]);
        let mut orchestrator = Orchestrator::new(
            project.path().to_path_buf(),
            "part.py".to_string(),
            Err("no configuration".to_string()),
            executor.clone(),
        );

        assert!(matches!(
            orchestrator.handle_reload(),
            OrchestratorResult::NoScript
        ));

        std::fs::write(&script_path, "ON_DISK = 1").unwrap();
        match orchestrator.handle_reload() {
            OrchestratorResult::ModelReady {
                model,
                origin: ModelOrigin::Reload,
            } => assert_eq!(model, "ON_DISK = 1"),
            _ => panic!("expected the on-disk model"),
        }
        assert_eq!(orchestrator.current_code(), "ON_DISK = 1");
        assert_eq!(executor.executed_code(), vec!["ON_DISK = 1"]);
    }

    #[tokio::test]
    async fn chat_without_ai_reports_the_reason_and_leaves_the_script_alone() {
        let project = tempfile::tempdir().unwrap();
        let script_path = project.path().join("part.py");
        std::fs::write(&script_path, "ORIGINAL_CODE = 1").unwrap();
        let mut orchestrator = Orchestrator::new(
            project.path().to_path_buf(),
            "part.py".to_string(),
            Err("DISTINCTIVE_REASON".to_string()),
            FakeExecutor::new([]),
        );
        match orchestrator.handle_chat("request").await {
            OrchestratorResult::BackendError(error) => {
                assert!(error.contains("DISTINCTIVE_REASON"));
            }
            _ => panic!("expected an unavailable-AI error"),
        }
        assert_eq!(
            std::fs::read_to_string(script_path).unwrap(),
            "ORIGINAL_CODE = 1"
        );
    }

    #[tokio::test]
    async fn runtime_faults_are_not_handed_to_the_ai_for_repair() {
        let (base_url, records, server) = recording_server(vec![
            completed("docs"),
            completed("```python\nCORRECT_CODE = True\n```\nSummary: Correct"),
        ])
        .await;
        let project = tempfile::tempdir().unwrap();
        let script_path = project.path().join("part.py");
        std::fs::write(&script_path, "ORIGINAL_CODE = 1").unwrap();
        let executor = FakeExecutor::scripted([Err(ExecutionFailure {
            message: "RUNTIME_FAULT".to_string(),
            retryable: false,
        })]);
        let mut orchestrator = orchestrator(project.path(), services(&base_url), executor.clone());

        let result = orchestrator.handle_chat("request").await;
        server.await.unwrap();
        match result {
            OrchestratorResult::ExecutionFailed { error, .. } => {
                assert_eq!(error, "RUNTIME_FAULT");
            }
            _ => panic!("expected a non-retried execution failure"),
        }
        assert_eq!(
            records.lock().unwrap().len(),
            2,
            "no retry request was sent"
        );
        assert_eq!(executor.executed_code(), vec!["CORRECT_CODE = True"]);
        assert_eq!(
            std::fs::read_to_string(script_path).unwrap(),
            "ORIGINAL_CODE = 1"
        );
    }

    #[tokio::test]
    async fn joined_two_consumer_request_reaches_script_and_final_result() {
        let lookup_text = "DISTINCTIVE_DOCUMENTATION_REFERENCE";
        let edit_text = "```python\nDISTINCTIVE_CODE = 42\n```\nSummary: Distinctive summary\nNotes: Distinctive message";
        let (base_url, records, server) =
            recording_server(vec![completed(lookup_text), completed(edit_text)]).await;
        let project = tempfile::tempdir().unwrap();
        let script_path = project.path().join("part.py");
        std::fs::write(&script_path, "ORIGINAL_CODE = 1").unwrap();
        let executor = FakeExecutor::new([Ok(())]);
        let mut orchestrator = orchestrator(project.path(), services(&base_url), executor.clone());
        orchestrator.push_message("genuinely previous request");

        let result = orchestrator.handle_chat("current request").await;
        server.await.unwrap();
        let records = records.lock().unwrap();

        assert_eq!(records.len(), 2);
        for record in records.iter() {
            assert_eq!(record.path, "/v1/responses");
            assert!(record.authenticated);
            assert_eq!(record.body["model"], "recording-model");
            assert_eq!(record.body["store"], false);
        }
        let lookup_input = records[0].input_text();
        assert!(
            records[0].body["instructions"]
                .as_str()
                .unwrap()
                .contains("API reference lookup tool")
        );
        assert!(lookup_input.contains("genuinely previous request"));
        assert!(lookup_input.contains("current request"));
        assert_eq!(lookup_input.matches("current request").count(), 1);
        assert!(lookup_input.contains("<build123d_documentation>"));

        let edit_input = records[1].input_text();
        assert!(
            records[1].body["instructions"]
                .as_str()
                .unwrap()
                .contains("build123d")
        );
        assert!(edit_input.contains("current request"));
        assert!(edit_input.contains("ORIGINAL_CODE = 1"));
        assert!(edit_input.contains(lookup_text));

        assert_eq!(executor.executed_code(), vec!["DISTINCTIVE_CODE = 42"]);
        assert_eq!(
            std::fs::read_to_string(&script_path).unwrap(),
            "DISTINCTIVE_CODE = 42"
        );
        match result {
            OrchestratorResult::ModelReady {
                model,
                origin: ModelOrigin::Edit(edit),
            } => {
                assert_eq!(model, "DISTINCTIVE_CODE = 42");
                assert_eq!(edit.response.code, "DISTINCTIVE_CODE = 42");
                assert_eq!(edit.response.summary, "Distinctive summary");
                assert!(edit.response.message.contains("Distinctive message"));
                assert_eq!(edit.trigger_message, "current request");
                assert_eq!(edit.script_path, script_path);
            }
            _ => panic!("expected successful orchestrator result"),
        }
    }

    #[tokio::test]
    async fn malformed_and_unparseable_edit_responses_preserve_original_script() {
        let invalid_responses = [
            "{not-json".to_string(),
            serde_json::json!({"status": "completed", "output": []}).to_string(),
            serde_json::json!({
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "no Python block"}]
                }]
            })
            .to_string(),
        ];

        for invalid in invalid_responses {
            let (base_url, _, server) = recording_server(vec![
                completed("No relevant documentation found."),
                ScriptedResponse {
                    status: 200,
                    body: invalid,
                },
            ])
            .await;
            let project = tempfile::tempdir().unwrap();
            let script_path = project.path().join("part.py");
            std::fs::write(&script_path, "ORIGINAL_CODE = 1").unwrap();
            let mut orchestrator =
                orchestrator(project.path(), services(&base_url), FakeExecutor::new([]));

            let result = orchestrator.handle_chat("request").await;
            server.await.unwrap();
            assert!(matches!(result, OrchestratorResult::BackendError(_)));
            assert_eq!(
                std::fs::read_to_string(script_path).unwrap(),
                "ORIGINAL_CODE = 1"
            );
        }
    }

    #[tokio::test]
    async fn structured_provider_error_reaches_consumer_without_credential() {
        let (base_url, _, server) = recording_server(vec![
            completed("No relevant documentation found."),
            provider_failure(&format!("rejected {FAKE_CREDENTIAL}")),
        ])
        .await;
        let project = tempfile::tempdir().unwrap();
        let script_path = project.path().join("part.py");
        std::fs::write(&script_path, "ORIGINAL_CODE = 1").unwrap();
        let mut orchestrator =
            orchestrator(project.path(), services(&base_url), FakeExecutor::new([]));

        let result = orchestrator.handle_chat("request").await;
        server.await.unwrap();
        match result {
            OrchestratorResult::BackendError(error) => {
                assert!(error.contains("scripted_failure"));
                assert!(error.contains("[REDACTED]"));
                assert!(!error.contains(FAKE_CREDENTIAL));
            }
            _ => panic!("expected provider failure"),
        }
        assert_eq!(
            std::fs::read_to_string(script_path).unwrap(),
            "ORIGINAL_CODE = 1"
        );
    }

    #[tokio::test]
    async fn documentation_failure_degrades_to_successful_edit_without_context() {
        let edit_text = "```python\nEDIT_WITHOUT_DOCS = True\n```\nSummary: Edit without lookup";
        let (base_url, records, server) = recording_server(vec![
            provider_failure("DISTINCTIVE_LOOKUP_FAILURE"),
            completed(edit_text),
        ])
        .await;
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("part.py"), "ORIGINAL_CODE = 1").unwrap();
        let mut orchestrator = orchestrator(
            project.path(),
            services(&base_url),
            FakeExecutor::new([Ok(())]),
        );

        let result = orchestrator.handle_chat("request").await;
        server.await.unwrap();
        assert!(matches!(result, OrchestratorResult::ModelReady { .. }));
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert!(
            !records[1]
                .input_text()
                .contains("Relevant build123d API reference")
        );
    }

    #[tokio::test]
    async fn traceback_retry_preserves_context_and_succeeds() {
        use cadmark_core::geometry::{FaceId, TopologyElement};
        use cadmark_core::ledger::{
            LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
        };

        let first_edit = "```python\nFIRST_BAD_CODE = True\n```\nSummary: First";
        let retry_edit = "```python\nRETRY_GOOD_CODE = True\n```\nSummary: Retry";
        let (base_url, records, server) = recording_server(vec![
            completed("DISTINCTIVE_RETRY_DOCS"),
            completed(first_edit),
            completed(retry_edit),
        ])
        .await;
        let project = tempfile::tempdir().unwrap();
        let script_path = project.path().join("part.py");
        std::fs::write(&script_path, "ORIGINAL_CODE = 1").unwrap();
        let executor = FakeExecutor::new([Err("DISTINCTIVE_TRACEBACK".to_string()), Ok(())]);
        let mut orchestrator = orchestrator(project.path(), services(&base_url), executor.clone());

        let mut identification = std::collections::HashMap::new();
        identification.insert("feature".to_string(), "DISTINCTIVE_FEATURE".to_string());
        let spatial_message_id = MessageId::new();
        let result = orchestrator
            .handle_spatial_comment(
                spatial_message_id,
                "retry request",
                GeometryContext {
                    element: TopologyElement::Face(FaceId(7)),
                    provenance: LedgerValue::Resolved(ProvenanceEntry {
                        source: SourceRef {
                            line: 23,
                            code: "DISTINCTIVE_SOURCE_SNIPPET".to_string(),
                        },
                        operation: SemanticOperation::Fillet,
                        operation_id: 9,
                        relation: ProvenanceRelation::Modified,
                    }),
                    identification,
                },
            )
            .await;
        server.await.unwrap();
        assert!(matches!(
            result,
            OrchestratorResult::ModelReady {
                origin: ModelOrigin::Edit(EditOutcome {
                    applied_spatial_message_id: Some(id),
                    ..
                }),
                ..
            } if id == spatial_message_id
        ));
        assert_eq!(
            executor.executed_code(),
            vec!["FIRST_BAD_CODE = True", "RETRY_GOOD_CODE = True"]
        );
        assert_eq!(
            std::fs::read_to_string(script_path).unwrap(),
            "RETRY_GOOD_CODE = True"
        );

        let records = records.lock().unwrap();
        let retry = &records[2];
        assert_eq!(retry.path, "/v1/responses");
        assert!(retry.authenticated);
        assert_eq!(retry.body["model"], "recording-model");
        let input = retry.input_text();
        for expected in [
            "DISTINCTIVE_TRACEBACK",
            "DISTINCTIVE_RETRY_DOCS",
            "ORIGINAL_CODE = 1",
            "retry request",
            "face 7",
            "line 23",
            "DISTINCTIVE_SOURCE_SNIPPET",
            "DISTINCTIVE_FEATURE",
        ] {
            assert!(input.contains(expected), "retry missing `{expected}`");
        }
    }

    #[tokio::test]
    async fn terminal_execution_failure_restores_original_script_and_reports_both_errors() {
        let (base_url, _, server) = recording_server(vec![
            completed("retry docs"),
            completed("```python\nFIRST_BAD = True\n```\nSummary: First"),
            completed("```python\nSECOND_BAD = True\n```\nSummary: Second"),
        ])
        .await;
        let project = tempfile::tempdir().unwrap();
        let script_path = project.path().join("part.py");
        std::fs::write(&script_path, "ORIGINAL_CODE = 1").unwrap();
        let mut orchestrator = orchestrator(
            project.path(),
            services(&base_url),
            FakeExecutor::new([
                Err("FIRST_EXECUTION_ERROR".to_string()),
                Err("SECOND_EXECUTION_ERROR".to_string()),
            ]),
        );

        let result = orchestrator.handle_chat("request").await;
        server.await.unwrap();
        match result {
            OrchestratorResult::ExecutionFailed { error, .. } => {
                assert!(error.contains("FIRST_EXECUTION_ERROR"));
                assert!(error.contains("SECOND_EXECUTION_ERROR"));
            }
            _ => panic!("expected terminal execution failure"),
        }
        assert_eq!(
            std::fs::read_to_string(script_path).unwrap(),
            "ORIGINAL_CODE = 1"
        );
    }

    #[tokio::test]
    async fn retry_provider_failure_restores_original_script() {
        let (base_url, _, server) = recording_server(vec![
            completed("retry docs"),
            completed("```python\nFIRST_BAD = True\n```\nSummary: First"),
            provider_failure(&format!("retry rejected {FAKE_CREDENTIAL}")),
        ])
        .await;
        let project = tempfile::tempdir().unwrap();
        let script_path = project.path().join("part.py");
        std::fs::write(&script_path, "ORIGINAL_CODE = 1").unwrap();
        let mut orchestrator = orchestrator(
            project.path(),
            services(&base_url),
            FakeExecutor::new([Err("FIRST_EXECUTION_ERROR".to_string())]),
        );

        let result = orchestrator.handle_chat("request").await;
        server.await.unwrap();
        match result {
            OrchestratorResult::BackendError(error) => {
                assert!(error.contains("Retry failed"));
                assert!(error.contains("FIRST_EXECUTION_ERROR"));
                assert!(!error.contains(FAKE_CREDENTIAL));
            }
            _ => panic!("expected retry provider failure"),
        }
        assert_eq!(
            std::fs::read_to_string(script_path).unwrap(),
            "ORIGINAL_CODE = 1"
        );
    }
}
