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
        results: VecDeque<Result<(), String>>,
        executed_code: Vec<String>,
    }

    impl FakeExecutor {
        fn new(results: impl IntoIterator<Item = Result<(), String>>) -> Self {
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
        fn execute(&self, script_path: &std::path::Path) -> Result<(), String> {
            let code = std::fs::read_to_string(script_path).unwrap();
            let mut state = self.state.lock().unwrap();
            state.executed_code.push(code);
            state.results.pop_front().unwrap_or(Ok(()))
        }
    }

    fn orchestrator(
        project_dir: &std::path::Path,
        services: AiServices,
        executor: FakeExecutor,
    ) -> Orchestrator {
        let mut orchestrator = Orchestrator::new(
            project_dir.to_path_buf(),
            "part.py".to_string(),
            services,
            Box::new(executor),
        );
        orchestrator.load_current_code().unwrap();
        orchestrator
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
        let lookup_input = records[0].body["input"].as_str().unwrap();
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

        let edit_input = records[1].body["input"].as_str().unwrap();
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
            OrchestratorResult::Success { response, .. } => {
                assert_eq!(response.code, "DISTINCTIVE_CODE = 42");
                assert_eq!(response.summary, "Distinctive summary");
                assert!(response.message.contains("Distinctive message"));
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
        assert!(matches!(result, OrchestratorResult::Success { .. }));
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert!(
            !records[1].body["input"]
                .as_str()
                .unwrap()
                .contains("Relevant build123d API reference")
        );
    }

    #[tokio::test]
    async fn traceback_retry_preserves_context_and_succeeds() {
        use cadmark_core::geometry::{FaceId, TopologyElement};

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
                    source_line: Some(23),
                    source_code: Some("DISTINCTIVE_SOURCE_SNIPPET".to_string()),
                    identification,
                },
            )
            .await;
        server.await.unwrap();
        assert!(matches!(
            result,
            OrchestratorResult::Success {
                applied_spatial_message_id: Some(id),
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
        let input = retry.body["input"].as_str().unwrap();
        for expected in [
            "DISTINCTIVE_TRACEBACK",
            "DISTINCTIVE_RETRY_DOCS",
            "ORIGINAL_CODE = 1",
            "retry request",
            "face #7",
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
