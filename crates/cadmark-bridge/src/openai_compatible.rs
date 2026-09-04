//! The one transport: the OpenAI Responses protocol, streamed, with tool
//! calls and image inputs. Every supported provider — OpenAI, Anthropic
//! and Google through a compatibility gateway, OpenRouter, a local model
//! server — is reached through this shape by changing the base URL and
//! model in settings.
//!
//! Streaming is how liveness is judged: a turn is alive while events
//! arrive, and a quiet stream is waited on (polling the cancel flag) for
//! as long as the user allows. There is no fixed request ceiling.

use std::fmt;
use std::time::Duration;

use base64::Engine;
use cadmark_core::cancellation::CancelFlag;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};

use crate::backend::{
    BackendError, DeltaSink, ImageData, ModelItem, ModelRequest, ModelResponse, RefusalCause,
    StreamDelta, ToolCall, ToolSpec, TurnModel,
};

const MAX_PROVIDER_MESSAGE_CHARS: usize = 500;

/// How often a quiet stream checks the cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(100);

pub(crate) struct Credential(String);

impl Credential {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    fn value(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Reusable provider client shared by every production AI consumer.
#[derive(Clone)]
pub struct OpenAiCompatibleClient {
    client: Client,
    responses_url: Url,
    model: String,
    credential: Option<std::sync::Arc<Credential>>,
    accepts_images: bool,
}

impl OpenAiCompatibleClient {
    pub(crate) fn new(
        responses_url: Url,
        model: String,
        credential: Option<Credential>,
        accepts_images: bool,
    ) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent(concat!("CADmark/", env!("CARGO_PKG_VERSION")))
            .build()?;

        Ok(Self {
            client,
            responses_url,
            model,
            credential: credential.map(std::sync::Arc::new),
            accepts_images,
        })
    }

    /// The model name sent with every request.
    pub fn model_name(&self) -> &str {
        &self.model
    }

    /// One request, its whole answer taken as text. For consumers with no
    /// tools and no need to stream: the documentation lookup, the smoke
    /// test.
    pub(crate) async fn request_text(
        &self,
        instructions: &str,
        input: &str,
    ) -> Result<String, BackendError> {
        let request = ModelRequest {
            instructions: instructions.to_string(),
            items: vec![ModelItem::User {
                text: input.to_string(),
                images: Vec::new(),
            }],
            tools: Vec::new(),
        };
        let mut sink = |_delta: StreamDelta| {};
        let response = self.stream(request, CancelFlag::new(), &mut sink).await?;
        if response.text.trim().is_empty() {
            return Err(BackendError::ParseError(
                "response has no assistant output text".to_string(),
            ));
        }
        Ok(response.text)
    }

    pub(crate) async fn smoke_test_exact_sentinel(
        &self,
        sentinel: &str,
    ) -> Result<(), BackendError> {
        let instructions = "Return exactly the requested sentinel and no other text.";
        let input = format!("Return exactly: {sentinel}");
        let output = self.request_text(instructions, &input).await?;
        if output.trim() == sentinel {
            Ok(())
        } else {
            Err(BackendError::ParseError(
                "live smoke response did not match the requested sentinel".to_string(),
            ))
        }
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancelFlag,
        sink: DeltaSink<'_>,
    ) -> Result<ModelResponse, BackendError> {
        let body = ResponsesRequest {
            model: &self.model,
            instructions: &request.instructions,
            input: request.items.iter().map(wire_item).collect(),
            tools: request.tools.iter().map(wire_tool).collect(),
            stream: true,
            store: false,
        };
        let mut http = self.client.post(self.responses_url.clone()).json(&body);
        if let Some(credential) = &self.credential {
            http = http.bearer_auth(credential.value());
        }

        let mut response = await_cancellable(http.send(), &cancel)
            .await?
            .map_err(map_transport_error)?;
        let status = response.status();
        if !status.is_success() {
            let body = await_cancellable(response.text(), &cancel)
                .await?
                .map_err(map_transport_error)?;
            return Err(non_success_error(status, &body, self.credential.as_deref()));
        }

        let mut parser = EventParser::default();
        let mut assembled = ResponseAssembly::default();
        loop {
            let chunk = await_cancellable(response.chunk(), &cancel)
                .await?
                .map_err(map_transport_error)?;
            let Some(chunk) = chunk else { break };
            for event in parser.push(&chunk) {
                assembled.apply(event, sink, self.credential.as_deref())?;
            }
        }
        assembled.finish()
    }
}

impl TurnModel for OpenAiCompatibleClient {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn accepts_images(&self) -> bool {
        self.accepts_images
    }

    fn respond<'a>(
        &'a self,
        request: ModelRequest,
        cancel: CancelFlag,
        sink: DeltaSink<'a>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ModelResponse, BackendError>> + Send + 'a>,
    > {
        Box::pin(self.stream(request, cancel, sink))
    }
}

impl fmt::Debug for OpenAiCompatibleClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleClient")
            .field("responses_url", &"[CONFIGURED]")
            .field("model", &"[CONFIGURED]")
            .field("credential", &self.credential)
            .field("accepts_images", &self.accepts_images)
            .finish_non_exhaustive()
    }
}

/// Wait on a provider future while polling the cancel flag. There is no
/// ceiling: the wait ends when the provider answers or the user cancels.
async fn await_cancellable<T>(
    future: impl std::future::Future<Output = T>,
    cancel: &CancelFlag,
) -> Result<T, BackendError> {
    tokio::pin!(future);
    loop {
        if cancel.is_cancelled() {
            return Err(BackendError::Cancelled);
        }
        match tokio::time::timeout(CANCEL_POLL, &mut future).await {
            Ok(value) => return Ok(value),
            Err(_elapsed) => {}
        }
    }
}

// ── Request wire shape ────────────────────────────────────────────────

/// Request body for the Responses API.
///
/// `input` is always the explicit item-array form rather than a bare
/// string: OpenAI accepts both, but Anthropic-translating gateways reject
/// the bare string with "cache_control cannot be set for empty text blocks".
#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    instructions: &'a str,
    input: Vec<WireItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    stream: bool,
    store: bool,
}

#[derive(Serialize)]
#[serde(untagged)]
enum WireItem {
    Message {
        role: &'static str,
        content: Vec<WireContent>,
    },
    FunctionCall {
        #[serde(rename = "type")]
        kind: &'static str,
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        #[serde(rename = "type")]
        kind: &'static str,
        call_id: String,
        output: String,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireContent {
    InputText { text: String },
    OutputText { text: String },
    InputImage { image_url: String },
}

#[derive(Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    description: String,
    parameters: serde_json::Value,
}

fn wire_item(item: &ModelItem) -> WireItem {
    match item {
        ModelItem::User { text, images } => {
            let mut content = vec![WireContent::InputText { text: text.clone() }];
            content.extend(images.iter().map(|image| WireContent::InputImage {
                image_url: data_url(image),
            }));
            WireItem::Message {
                role: "user",
                content,
            }
        }
        ModelItem::Assistant { text } => WireItem::Message {
            role: "assistant",
            content: vec![WireContent::OutputText { text: text.clone() }],
        },
        ModelItem::ToolCall(call) => WireItem::FunctionCall {
            kind: "function_call",
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.to_string(),
        },
        ModelItem::ToolResult { call_id, output } => WireItem::FunctionCallOutput {
            kind: "function_call_output",
            call_id: call_id.clone(),
            output: output.clone(),
        },
    }
}

fn wire_tool(tool: &ToolSpec) -> WireTool {
    WireTool {
        kind: "function",
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.parameters.clone(),
    }
}

fn data_url(image: &ImageData) -> String {
    format!(
        "data:{};base64,{}",
        image.media_type,
        base64::engine::general_purpose::STANDARD.encode(&image.bytes)
    )
}

// ── Response stream ───────────────────────────────────────────────────

/// Splits a server-sent-events byte stream into JSON events, across chunk
/// boundaries.
#[derive(Default)]
struct EventParser {
    buffer: Vec<u8>,
}

impl EventParser {
    fn push(&mut self, chunk: &[u8]) -> Vec<serde_json::Value> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        // Events are separated by a blank line; a partial event stays
        // buffered until its terminator arrives.
        while let Some(end) = find_event_end(&self.buffer) {
            let (event_bytes, separator_len) = end;
            let event: Vec<u8> = self.buffer.drain(..event_bytes + separator_len).collect();
            let text = String::from_utf8_lossy(&event[..event_bytes]);
            let data: String = text
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            if let Ok(value) = serde_json::from_str(&data) {
                events.push(value);
            }
        }
        events
    }
}

fn find_event_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(a), Some(b)) if b < a => Some((b, 4)),
        (Some(a), _) => Some((a, 2)),
        (None, Some(b)) => Some((b, 4)),
        (None, None) => None,
    }
}

/// Builds the response from the event stream.
#[derive(Default)]
struct ResponseAssembly {
    text: String,
    /// Tool calls by output index, their argument text accumulating.
    calls: Vec<PendingCall>,
    completed: bool,
    failed: Option<String>,
}

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

impl ResponseAssembly {
    fn apply(
        &mut self,
        event: serde_json::Value,
        sink: DeltaSink<'_>,
        credential: Option<&Credential>,
    ) -> Result<(), BackendError> {
        let kind = event["type"].as_str().unwrap_or_default();
        match kind {
            "response.output_text.delta" => {
                if let Some(delta) = event["delta"].as_str() {
                    self.text.push_str(delta);
                    sink(StreamDelta::Text(delta.to_string()));
                }
            }
            "response.output_item.added" => {
                let item = &event["item"];
                if item["type"].as_str() == Some("function_call") {
                    let name = item["name"].as_str().unwrap_or_default().to_string();
                    sink(StreamDelta::ToolCallStarted { name: name.clone() });
                    self.calls.push(PendingCall {
                        id: item["call_id"].as_str().unwrap_or_default().to_string(),
                        name,
                        arguments: item["arguments"].as_str().unwrap_or_default().to_string(),
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                if let (Some(call), Some(delta)) = (self.calls.last_mut(), event["delta"].as_str()) {
                    call.arguments.push_str(delta);
                }
            }
            "response.output_item.done" => {
                // The completed item carries the whole call: authoritative
                // over any deltas, and the only form some gateways send.
                let item = &event["item"];
                if item["type"].as_str() == Some("function_call") {
                    let id = item["call_id"].as_str().unwrap_or_default();
                    let arguments = item["arguments"].as_str().unwrap_or_default().to_string();
                    match self.calls.iter_mut().find(|call| call.id == id) {
                        Some(call) => call.arguments = arguments,
                        None => self.calls.push(PendingCall {
                            id: id.to_string(),
                            name: item["name"].as_str().unwrap_or_default().to_string(),
                            arguments,
                        }),
                    }
                }
            }
            "response.completed" => self.completed = true,
            "response.failed" | "response.incomplete" => {
                let error = &event["response"]["error"];
                self.failed = Some(format_provider_error(
                    None,
                    &ProviderError {
                        kind: error["type"].as_str().map(str::to_string),
                        code: error["code"].as_str().map(str::to_string),
                        message: error["message"].as_str().map(str::to_string),
                    },
                    credential,
                ));
            }
            "error" => {
                self.failed = Some(format_provider_error(
                    None,
                    &ProviderError {
                        kind: event["type"].as_str().map(str::to_string),
                        code: event["code"].as_str().map(str::to_string),
                        message: event["message"].as_str().map(str::to_string),
                    },
                    credential,
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> Result<ModelResponse, BackendError> {
        if let Some(failure) = self.failed {
            return Err(BackendError::RequestFailed(failure));
        }
        if !self.completed {
            return Err(BackendError::ParseError(
                "the stream ended before the response completed".to_string(),
            ));
        }
        let mut tool_calls = Vec::with_capacity(self.calls.len());
        for call in self.calls {
            let arguments = serde_json::from_str(&call.arguments).map_err(|error| {
                BackendError::ParseError(format!(
                    "tool call {} carried unreadable arguments: {error}",
                    call.name
                ))
            })?;
            tool_calls.push(ToolCall {
                id: call.id,
                name: call.name,
                arguments,
            });
        }
        Ok(ModelResponse {
            text: self.text,
            tool_calls,
        })
    }
}

// ── Errors ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ProviderErrorEnvelope {
    error: Option<ProviderError>,
}

#[derive(Deserialize)]
struct ProviderError {
    #[serde(rename = "type")]
    kind: Option<String>,
    code: Option<String>,
    message: Option<String>,
}

fn map_transport_error(error: reqwest::Error) -> BackendError {
    if error.is_connect() {
        BackendError::Unavailable("could not connect to the configured provider".to_string())
    } else {
        BackendError::RequestFailed("provider transport failed".to_string())
    }
}

/// A non-success status, read for its cause so the user learns why.
fn non_success_error(
    status: StatusCode,
    body: &str,
    credential: Option<&Credential>,
) -> BackendError {
    let provider_error = serde_json::from_str::<ProviderErrorEnvelope>(body)
        .ok()
        .and_then(|envelope| envelope.error)
        .unwrap_or(ProviderError {
            kind: None,
            code: None,
            message: None,
        });
    let detail = format_provider_error(Some(status), &provider_error, credential);
    match refusal_cause(status, &provider_error) {
        Some(cause) => BackendError::Refused { cause, detail },
        None => BackendError::RequestFailed(detail),
    }
}

fn refusal_cause(status: StatusCode, error: &ProviderError) -> Option<RefusalCause> {
    let code = error.code.as_deref().unwrap_or_default().to_ascii_lowercase();
    let kind = error.kind.as_deref().unwrap_or_default().to_ascii_lowercase();
    let message = error
        .message
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mentions = |needle: &str| code.contains(needle) || kind.contains(needle) || message.contains(needle);
    if status == StatusCode::TOO_MANY_REQUESTS
        || mentions("rate_limit")
        || mentions("usage_limit")
        || mentions("quota")
        || mentions("cooldown")
        || mentions("insufficient_quota")
    {
        Some(RefusalCause::UsageLimit)
    } else if status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
        || mentions("authentication")
        || mentions("invalid_api_key")
    {
        Some(RefusalCause::Authentication)
    } else if status == StatusCode::NOT_FOUND && mentions("model") || mentions("model_not_found") {
        Some(RefusalCause::UnknownModel)
    } else {
        None
    }
}

fn format_provider_error(
    status: Option<StatusCode>,
    error: &ProviderError,
    credential: Option<&Credential>,
) -> String {
    let mut parts = Vec::new();
    if let Some(status) = status {
        parts.push(format!("HTTP {}", status.as_u16()));
    }
    if let Some(kind) = error.kind.as_deref().filter(|value| !value.is_empty()) {
        parts.push(format!("type {kind}"));
    }
    if let Some(code) = error.code.as_deref().filter(|value| !value.is_empty()) {
        parts.push(format!("code {code}"));
    }
    if let Some(message) = error.message.as_deref().filter(|value| !value.is_empty()) {
        let redacted = credential
            .map(|credential| message.replace(credential.value(), "[REDACTED]"))
            .unwrap_or_else(|| message.to_string());
        parts.push(truncate(&redacted, MAX_PROVIDER_MESSAGE_CHARS));
    }
    if parts.is_empty() {
        "provider request failed".to_string()
    } else {
        parts.join(": ")
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let shortened: String = characters.by_ref().take(max_chars).collect();
    if characters.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

#[cfg(test)]
pub(crate) mod recording {
    //! A scripted provider on a local port: records each request's body
    //! and answers with the scripted response. Shared by every test that
    //! needs the wire, so the request shape is asserted in one idiom.

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Debug)]
    pub(crate) struct RecordedRequest {
        pub(crate) path: String,
        pub(crate) authenticated: bool,
        pub(crate) body: serde_json::Value,
    }

    impl RecordedRequest {
        /// The text of the `index`th input item.
        pub(crate) fn input_text(&self, index: usize) -> &str {
            self.body["input"][index]["content"][0]["text"]
                .as_str()
                .expect("recorded request carries message-array input")
        }
    }

    pub(crate) struct ScriptedResponse {
        pub(crate) status: u16,
        pub(crate) body: String,
        /// Sent as server-sent events (streamed) rather than one JSON body.
        pub(crate) streamed: bool,
        pub(crate) delay: Option<Duration>,
    }

    /// A streamed completed response carrying `text`.
    pub(crate) fn completed(text: &str) -> ScriptedResponse {
        ScriptedResponse {
            status: 200,
            body: events(&[
                serde_json::json!({"type": "response.output_text.delta", "delta": text}),
                serde_json::json!({"type": "response.completed", "response": {"status": "completed"}}),
            ]),
            streamed: true,
            delay: None,
        }
    }

    /// A streamed completed response carrying one tool call.
    pub(crate) fn tool_call(name: &str, arguments: serde_json::Value) -> ScriptedResponse {
        ScriptedResponse {
            status: 200,
            body: events(&[
                serde_json::json!({"type": "response.output_item.added", "output_index": 0,
                    "item": {"type": "function_call", "call_id": "call_1", "name": name, "arguments": ""}}),
                serde_json::json!({"type": "response.function_call_arguments.delta", "output_index": 0,
                    "delta": arguments.to_string()}),
                serde_json::json!({"type": "response.output_item.done", "output_index": 0,
                    "item": {"type": "function_call", "call_id": "call_1", "name": name, "arguments": arguments.to_string()}}),
                serde_json::json!({"type": "response.completed", "response": {"status": "completed"}}),
            ]),
            streamed: true,
            delay: None,
        }
    }

    pub(crate) fn provider_failure(status: u16, kind: &str, code: &str, message: &str) -> ScriptedResponse {
        ScriptedResponse {
            status,
            body: serde_json::json!({
                "error": {"type": kind, "code": code, "message": message}
            })
            .to_string(),
            streamed: false,
            delay: None,
        }
    }

    pub(crate) fn events(events: &[serde_json::Value]) -> String {
        events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>()
            + "data: [DONE]\n\n"
    }

    pub(crate) async fn recording_server(
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
                if let Some(delay) = response.delay {
                    tokio::time::sleep(delay).await;
                }
                let reason = if response.status == 200 { "OK" } else { "Error" };
                let content_type = if response.streamed {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let wire = format!(
                    "HTTP/1.1 {} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    response.body.len(),
                    response.body
                );
                let _ = stream.write_all(wire.as_bytes()).await;
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
}

#[cfg(test)]
mod tests {
    use super::recording::*;
    use super::*;

    fn client(base_url: &str, credential: Option<&str>) -> OpenAiCompatibleClient {
        OpenAiCompatibleClient::new(
            Url::parse(&format!("{base_url}/responses")).unwrap(),
            "configured-model".to_string(),
            credential.map(|value| Credential::new(value.to_string())),
            true,
        )
        .unwrap()
    }

    #[test]
    fn event_parser_splits_events_across_chunk_boundaries() {
        let mut parser = EventParser::default();
        let whole = events(&[
            serde_json::json!({"type": "a"}),
            serde_json::json!({"type": "b"}),
        ]);
        let (first, second) = whole.split_at(whole.len() / 2);
        let mut seen = parser.push(first.as_bytes());
        seen.extend(parser.push(second.as_bytes()));
        let kinds: Vec<_> = seen.iter().map(|event| event["type"].as_str().unwrap().to_string()).collect();
        assert_eq!(kinds, ["a", "b"]);
    }

    #[test]
    fn refusals_are_classified_by_status_and_by_message() {
        let error = |code: &str, message: &str| ProviderError {
            kind: None,
            code: Some(code.to_string()),
            message: Some(message.to_string()),
        };
        assert_eq!(
            refusal_cause(StatusCode::TOO_MANY_REQUESTS, &error("", "")),
            Some(RefusalCause::UsageLimit)
        );
        assert_eq!(
            refusal_cause(StatusCode::BAD_GATEWAY, &error("model_cooldown", "usage limit reached")),
            Some(RefusalCause::UsageLimit)
        );
        assert_eq!(
            refusal_cause(StatusCode::UNAUTHORIZED, &error("", "")),
            Some(RefusalCause::Authentication)
        );
        assert_eq!(
            refusal_cause(StatusCode::NOT_FOUND, &error("model_not_found", "no such model")),
            Some(RefusalCause::UnknownModel)
        );
        assert_eq!(
            refusal_cause(StatusCode::INTERNAL_SERVER_ERROR, &error("boom", "exploded")),
            None
        );
    }

    #[test]
    fn provider_error_redacts_before_bounding() {
        let credential = Credential::new("fake-secret".to_string());
        let error = ProviderError {
            kind: Some("authentication_error".to_string()),
            code: Some("invalid_key".to_string()),
            message: Some(format!("bad fake-secret {}", "x".repeat(600))),
        };
        let formatted =
            format_provider_error(Some(StatusCode::UNAUTHORIZED), &error, Some(&credential));
        assert!(!formatted.contains("fake-secret"));
        assert!(formatted.contains("[REDACTED]"));
        assert!(formatted.chars().count() < 600);
    }

    #[tokio::test]
    async fn sends_the_streamed_responses_contract_and_assembles_text_and_tool_calls() {
        let (base_url, records, server) = recording_server(vec![
            completed("distinctive"),
            tool_call("run_script", serde_json::json!({"code": "x = 1", "summary": "Set x"})),
        ])
        .await;
        let client = client(&base_url, Some("fake-token"));

        let mut deltas = Vec::new();
        let mut sink = |delta: StreamDelta| deltas.push(delta);
        let request = ModelRequest {
            instructions: "system content".into(),
            items: vec![
                ModelItem::User {
                    text: "user content".into(),
                    images: vec![ImageData {
                        media_type: "image/png".into(),
                        bytes: vec![1, 2, 3],
                    }],
                },
                ModelItem::Assistant {
                    text: "earlier reply".into(),
                },
                ModelItem::ToolCall(ToolCall {
                    id: "call_0".into(),
                    name: "lookup_docs".into(),
                    arguments: serde_json::json!({"query": "fillet"}),
                }),
                ModelItem::ToolResult {
                    call_id: "call_0".into(),
                    output: "fillet(objects, radius)".into(),
                },
            ],
            tools: crate::tools::tools_for(true),
        };
        let first = client
            .stream(request.clone(), CancelFlag::new(), &mut sink)
            .await
            .unwrap();
        assert_eq!(first.text, "distinctive");
        assert!(first.tool_calls.is_empty());
        assert_eq!(deltas, [StreamDelta::Text("distinctive".into())]);

        let second = client
            .stream(request, CancelFlag::new(), &mut sink)
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(second.tool_calls.len(), 1);
        assert_eq!(second.tool_calls[0].name, "run_script");
        assert_eq!(second.tool_calls[0].arguments["code"], "x = 1");
        assert!(matches!(deltas.last(), Some(StreamDelta::ToolCallStarted { name }) if name == "run_script"));

        let records = records.lock().unwrap();
        let body = &records[0].body;
        assert_eq!(records[0].path, "/v1/responses");
        assert!(records[0].authenticated);
        assert_eq!(body["model"], "configured-model");
        assert_eq!(body["instructions"], "system content");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
        assert!(body["input"][0]["content"][1]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert_eq!(body["input"][1]["role"], "assistant");
        assert_eq!(body["input"][2]["type"], "function_call");
        assert_eq!(body["input"][2]["call_id"], "call_0");
        assert_eq!(body["input"][3]["type"], "function_call_output");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "run_script");
        assert_eq!(body["tools"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn refusals_reach_the_caller_by_cause_without_the_credential() {
        let secret = "fake-provider-secret";
        let (base_url, _, server) = recording_server(vec![provider_failure(
            429,
            "rate_limit_error",
            "model_cooldown",
            &format!("usage limit reached for {secret}"),
        )])
        .await;
        let client = client(&base_url, Some(secret));
        let error = client
            .request_text("instructions", "input")
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(matches!(
            error,
            BackendError::Refused {
                cause: RefusalCause::UsageLimit,
                ..
            }
        ));
        assert!(!error.to_string().contains(secret));
        assert!(error.to_string().contains("[REDACTED]"));
        assert!(error.to_string().contains("cooling down"));
    }

    #[tokio::test]
    async fn a_stream_that_never_completes_is_a_parse_error_and_a_failed_response_is_a_request_error() {
        let (base_url, _, server) = recording_server(vec![
            ScriptedResponse {
                status: 200,
                body: events(&[serde_json::json!({"type": "response.output_text.delta", "delta": "half"})]),
                streamed: true,
                delay: None,
            },
            ScriptedResponse {
                status: 200,
                body: events(&[serde_json::json!({"type": "response.failed",
                    "response": {"error": {"type": "server_error", "code": "overloaded", "message": "try later"}}})]),
                streamed: true,
                delay: None,
            },
        ])
        .await;
        let client = client(&base_url, None);
        assert!(matches!(
            client.request_text("i", "x").await,
            Err(BackendError::ParseError(_))
        ));
        let failed = client.request_text("i", "x").await.unwrap_err();
        server.await.unwrap();
        assert!(matches!(failed, BackendError::RequestFailed(_)));
        assert!(failed.to_string().contains("overloaded"));
    }

    #[tokio::test]
    async fn a_quiet_provider_is_waited_on_until_the_user_cancels() {
        let (base_url, _, server) = recording_server(vec![ScriptedResponse {
            status: 200,
            body: completed("late").body,
            streamed: true,
            delay: Some(Duration::from_secs(30)),
        }])
        .await;
        let client = client(&base_url, None);
        let cancel = CancelFlag::new();
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            canceller.cancel();
        });
        let started = std::time::Instant::now();
        let mut sink = |_delta: StreamDelta| {};
        let request = ModelRequest {
            instructions: "i".into(),
            items: vec![ModelItem::User {
                text: "x".into(),
                images: Vec::new(),
            }],
            tools: Vec::new(),
        };
        let error = client.stream(request, cancel, &mut sink).await.unwrap_err();
        assert_eq!(error, BackendError::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(5));
        server.abort();
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_is_reported_as_unavailable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = client(&format!("http://{address}/v1"), None);
        assert!(matches!(
            client.request_text("instructions", "input").await,
            Err(BackendError::Unavailable(_))
        ));
    }
}
