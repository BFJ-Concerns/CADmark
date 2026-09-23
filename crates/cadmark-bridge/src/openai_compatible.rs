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
        cancel: CancelFlag,
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
        let response = self.stream(request, cancel, &mut sink).await?;
        if response.reached_output_limit {
            return Err(BackendError::RequestFailed(
                "the response reached the provider's output limit before it was finished"
                    .to_string(),
            ));
        }
        if response.text.trim().is_empty() {
            return Err(BackendError::ParseError(
                "response has no assistant output text".to_string(),
            ));
        }
        Ok(response.text)
    }

    /// The model's context window as the endpoint advertises it, or `None`
    /// when it advertises nothing CADmark recognises. Compatible endpoints
    /// share no one field for this, so the well-known spellings are all
    /// read: the model's own record at `models/{model}` first, then the
    /// model list. Any failure reads as "not advertised" rather than an
    /// error: the manual setting stands in, and a probe must never stop a
    /// project opening.
    pub async fn context_window(&self, cancel: CancelFlag) -> Option<usize> {
        let models_url = self.models_url()?;
        let direct = {
            let mut url = models_url.clone();
            let path = url.path().trim_end_matches('/').to_string();
            url.set_path(&format!("{path}/{}", self.model));
            url
        };
        if let Some(window) = self
            .fetch_json(direct, &cancel)
            .await
            .and_then(|record| advertised_context_window(&record))
        {
            return Some(window);
        }
        let list = self.fetch_json(models_url, &cancel).await?;
        list["data"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|record| record["id"].as_str() == Some(self.model.as_str()))
            .and_then(advertised_context_window)
    }

    /// The `models` endpoint beside `responses`.
    fn models_url(&self) -> Option<Url> {
        let mut url = self.responses_url.clone();
        let path = url.path().trim_end_matches('/');
        let parent = path.strip_suffix("/responses")?.to_string();
        url.set_path(&format!("{parent}/models"));
        Some(url)
    }

    /// One authenticated GET, its body read as JSON when the status is a
    /// success and the body is of a sane size; anything else is `None`.
    async fn fetch_json(&self, url: Url, cancel: &CancelFlag) -> Option<serde_json::Value> {
        const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
        let mut http = self.client.get(url);
        if let Some(credential) = &self.credential {
            http = http.bearer_auth(credential.value());
        }
        let response = await_cancellable(http.send(), cancel).await.ok()?.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body = await_cancellable(response.bytes(), cancel)
            .await
            .ok()?
            .ok()?;
        if body.len() > MAX_BODY_BYTES {
            return None;
        }
        serde_json::from_slice(&body).ok()
    }

    pub(crate) async fn smoke_test_exact_sentinel(
        &self,
        sentinel: &str,
    ) -> Result<(), BackendError> {
        let instructions = "Return exactly the requested sentinel and no other text.";
        let input = format!("Return exactly: {sentinel}");
        let output = self
            .request_text(instructions, &input, CancelFlag::new())
            .await?;
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
            for event in parser.push(&chunk)? {
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

/// The context window a model record advertises, under whichever of the
/// spellings compatible endpoints use: OpenAI-style `context_window`,
/// OpenRouter's `context_length` (top-level or under `top_provider`),
/// LiteLLM's `max_input_tokens`, llama.cpp's `meta.n_ctx_train`, and
/// Ollama-style `model_info` entries ending in `.context_length`. A value
/// below what any usable model has is ignored as a mistake.
fn advertised_context_window(record: &serde_json::Value) -> Option<usize> {
    const MIN_PLAUSIBLE: u64 = 1_024;
    let direct = [
        "context_window",
        "context_length",
        "max_context_length",
        "context_window_tokens",
        "max_input_tokens",
        "n_ctx",
    ]
    .into_iter()
    .filter_map(|key| record[key].as_u64());
    let nested = [
        &record["top_provider"]["context_length"],
        &record["meta"]["n_ctx_train"],
        &record["metadata"]["context_length"],
    ]
    .into_iter()
    .filter_map(serde_json::Value::as_u64);
    let model_info = record["model_info"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| key.ends_with(".context_length"))
        .filter_map(|(_, value)| value.as_u64());
    direct
        .chain(nested)
        .chain(model_info)
        .find(|value| *value >= MIN_PLAUSIBLE)
        .and_then(|value| usize::try_from(value).ok())
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
    /// Take the events completed by `chunk`. A frame whose data is not JSON
    /// is a parse failure, not a gap: a stream with a hole in it must not
    /// finish as a shorter response that looks complete.
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<serde_json::Value>, BackendError> {
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
            match serde_json::from_str(&data) {
                Ok(value) => events.push(value),
                // The error names the position only; the frame's text is
                // the provider's and may carry anything, so it stays out
                // of what the user is shown.
                Err(error) => {
                    return Err(BackendError::ParseError(format!(
                        "the provider sent a malformed stream event ({} bytes): {error}",
                        data.len()
                    )));
                }
            }
        }
        Ok(events)
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

/// The incomplete-response reason a provider gives when the model reached
/// the output-token limit.
const OUTPUT_LIMIT_REASON: &str = "max_output_tokens";

/// Builds the response from the event stream.
#[derive(Default)]
struct ResponseAssembly {
    /// The reply text exactly as it was passed to the sink: every message
    /// item's text in order, a blank line between items.
    text: String,
    /// Each message item's own text so far, by output index, so a completed
    /// item can fill in whatever its deltas did not deliver.
    messages: Vec<(Option<u64>, String)>,
    /// Tool calls by output index, their argument text accumulating.
    calls: Vec<PendingCall>,
    completed: bool,
    reached_output_limit: bool,
    failed: Option<BackendError>,
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
        let output_index = event["output_index"].as_u64();
        match kind {
            "response.output_text.delta" => {
                if let Some(delta) = event["delta"].as_str() {
                    self.push_text(output_index, delta, sink);
                }
            }
            "response.output_text.done" => {
                if let Some(text) = event["text"].as_str() {
                    self.settle_text(output_index, text, sink);
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
                if let (Some(call), Some(delta)) = (self.calls.last_mut(), event["delta"].as_str())
                {
                    call.arguments.push_str(delta);
                }
            }
            "response.output_item.done" => {
                // The completed item carries the whole call: authoritative
                // over any deltas, and the only form some gateways send.
                let item = &event["item"];
                match item["type"].as_str() {
                    Some("function_call") => {
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
                    Some("message") => {
                        let text: String = item["content"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|part| part["type"].as_str() == Some("output_text"))
                            .filter_map(|part| part["text"].as_str())
                            .collect();
                        self.settle_text(output_index, &text, sink);
                    }
                    _ => {}
                }
            }
            "response.completed" => self.completed = true,
            "response.incomplete" => {
                let reason = event["response"]["incomplete_details"]["reason"].as_str();
                if reason == Some(OUTPUT_LIMIT_REASON) {
                    self.reached_output_limit = true;
                } else {
                    self.failed = Some(stream_error(
                        &ProviderError {
                            kind: None,
                            code: reason.map(str::to_string),
                            message: Some(
                                "the provider ended the response before it was finished"
                                    .to_string(),
                            ),
                        },
                        credential,
                    ));
                }
            }
            "response.failed" => {
                self.failed = Some(stream_error(
                    &ProviderError::from_json(&event["response"]["error"]),
                    credential,
                ));
            }
            "error" => {
                // OpenAI puts the fields on the event itself; a gateway
                // relaying an upstream error nests them under `error`. On
                // the event, `type` is the event's own type, not the cause.
                let error = if event["error"].is_object() {
                    ProviderError::from_json(&event["error"])
                } else {
                    ProviderError {
                        kind: None,
                        ..ProviderError::from_json(&event)
                    }
                };
                self.failed = Some(stream_error(&error, credential));
            }
            _ => {}
        }
        Ok(())
    }

    /// Add `piece` to the text of the message item at `index` (the latest
    /// item when the event names none) and pass it on. The first text of a
    /// later item is set off from the earlier items' by a blank line.
    fn push_text(&mut self, index: Option<u64>, piece: &str, sink: DeltaSink<'_>) {
        if piece.is_empty() {
            return;
        }
        let existing = match index {
            Some(_) => self.messages.iter().position(|(item, _)| *item == index),
            None => self.messages.len().checked_sub(1),
        };
        let position = existing.unwrap_or_else(|| {
            self.messages.push((index, String::new()));
            self.messages.len() - 1
        });
        let mut delta = String::new();
        if self.messages[position].1.is_empty() && !self.text.is_empty() {
            delta.push_str("\n\n");
        }
        delta.push_str(piece);
        self.messages[position].1.push_str(piece);
        self.text.push_str(&delta);
        sink(StreamDelta::Text(delta));
    }

    /// A message item's whole text, as its completion reports it. Text the
    /// deltas did not deliver — all of it, from a gateway that sends none —
    /// is passed on now. Text the deltas did deliver stands as delivered,
    /// since the user has already read it.
    fn settle_text(&mut self, index: Option<u64>, whole: &str, sink: DeltaSink<'_>) {
        let streamed = match index {
            Some(_) => self.messages.iter().find(|(item, _)| *item == index),
            None => self.messages.last(),
        }
        .map(|(_, text)| text.as_str())
        .unwrap_or_default();
        if let Some(rest) = whole.strip_prefix(streamed) {
            self.push_text(index, rest, sink);
        }
    }

    fn finish(self) -> Result<ModelResponse, BackendError> {
        if let Some(failure) = self.failed {
            return Err(failure);
        }
        if !self.completed && !self.reached_output_limit {
            return Err(BackendError::ParseError(
                "the stream ended before the response completed".to_string(),
            ));
        }
        let mut tool_calls = Vec::with_capacity(self.calls.len());
        for call in self.calls {
            let arguments = match serde_json::from_str(&call.arguments) {
                Ok(arguments) => arguments,
                // The limit fell while this call was being written.
                Err(_) if self.reached_output_limit => continue,
                Err(error) => {
                    return Err(BackendError::ParseError(format!(
                        "tool call {} carried unreadable arguments: {error}",
                        call.name
                    )));
                }
            };
            tool_calls.push(ToolCall {
                id: call.id,
                name: call.name,
                arguments,
            });
        }
        Ok(ModelResponse {
            text: self.text,
            tool_calls,
            reached_output_limit: self.reached_output_limit,
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

impl ProviderError {
    /// The fields of an error object as a stream event carries it.
    fn from_json(error: &serde_json::Value) -> Self {
        let field = |name: &str| error[name].as_str().map(str::to_string);
        Self {
            kind: field("type"),
            code: field("code"),
            message: field("message"),
        }
    }
}

/// An error the provider reported inside a stream it had accepted: a
/// refusal by cause where its words name one, as a refusal before the
/// stream would be.
fn stream_error(error: &ProviderError, credential: Option<&Credential>) -> BackendError {
    let detail = format_provider_error(None, error, credential);
    match refusal_cause(StatusCode::OK, error) {
        Some(cause) => BackendError::Refused { cause, detail },
        None => BackendError::RequestFailed(detail),
    }
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
    let code = error
        .code
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let kind = error
        .kind
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let message = error
        .message
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mentions =
        |needle: &str| code.contains(needle) || kind.contains(needle) || message.contains(needle);
    if status == StatusCode::TOO_MANY_REQUESTS
        || mentions("rate_limit")
        || mentions("usage_limit")
        || mentions("quota")
        || mentions("cooldown")
        || mentions("insufficient_quota")
    {
        Some(RefusalCause::UsageLimit)
    } else if status == StatusCode::SERVICE_UNAVAILABLE
        || status.as_u16() == 529
        || mentions("overloaded")
        || mentions("server is busy")
        || mentions("no slots available")
        || mentions("server_busy")
    {
        Some(RefusalCause::Overloaded)
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

    pub(crate) fn provider_failure(
        status: u16,
        kind: &str,
        code: &str,
        message: &str,
    ) -> ScriptedResponse {
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
                let reason = if response.status == 200 {
                    "OK"
                } else {
                    "Error"
                };
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
        // A GET carries no body and no length.
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
            })
            .unwrap_or(0);
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
        let body = if content_length == 0 {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap()
        };
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
        let mut seen = parser.push(first.as_bytes()).unwrap();
        seen.extend(parser.push(second.as_bytes()).unwrap());
        let kinds: Vec<_> = seen
            .iter()
            .map(|event| event["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["a", "b"]);
    }

    /// Run `events` through a fresh assembly, returning the response and
    /// the text the sink was handed.
    fn assemble(
        events: &[serde_json::Value],
        credential: Option<&Credential>,
    ) -> (Result<ModelResponse, BackendError>, String) {
        let mut assembly = ResponseAssembly::default();
        let mut streamed = String::new();
        let mut sink = |delta: StreamDelta| {
            if let StreamDelta::Text(text) = delta {
                streamed.push_str(&text);
            }
        };
        for event in events {
            assembly
                .apply(event.clone(), &mut sink, credential)
                .unwrap();
        }
        (assembly.finish(), streamed)
    }

    fn call_done(index: u64, id: &str, arguments: &str) -> serde_json::Value {
        serde_json::json!({"type": "response.output_item.done", "output_index": index,
            "item": {"type": "function_call", "call_id": id, "name": "run_script", "arguments": arguments}})
    }

    #[test]
    fn a_response_cut_off_at_the_output_limit_keeps_its_text_and_every_whole_call() {
        let (response, streamed) = assemble(
            &[
                serde_json::json!({"type": "response.output_text.delta", "output_index": 0,
                    "delta": "Building the duct now."}),
                call_done(1, "call_1", r#"{"code": "x = 1", "summary": "first"}"#),
                call_done(2, "call_2", r#"{"code": "y = "#),
                serde_json::json!({"type": "response.incomplete", "response": {"status": "incomplete",
                    "error": null, "incomplete_details": {"reason": "max_output_tokens"}}}),
            ],
            None,
        );
        let response = response.unwrap();
        assert!(response.reached_output_limit);
        assert_eq!(response.text, "Building the duct now.");
        assert_eq!(streamed, response.text);
        let ids: Vec<_> = response.tool_calls.iter().map(|call| &call.id).collect();
        assert_eq!(ids, ["call_1"], "the call cut off mid-arguments is dropped");
    }

    #[test]
    fn a_response_ended_early_for_another_reason_fails_naming_the_reason() {
        let (response, streamed) = assemble(
            &[
                serde_json::json!({"type": "response.output_text.delta", "delta": "Half"}),
                serde_json::json!({"type": "response.incomplete", "response": {
                    "incomplete_details": {"reason": "content_filter"}}}),
            ],
            None,
        );
        let error = response.unwrap_err();
        assert!(matches!(error, BackendError::RequestFailed(_)), "{error:?}");
        assert!(error.to_string().contains("content_filter"), "{error}");
        assert_eq!(streamed, "Half", "what streamed was still passed on");
    }

    #[test]
    fn an_error_event_is_read_from_its_nested_error_or_its_own_fields() {
        let credential = Credential::new("fake-secret".to_string());
        let (nested, _) = assemble(
            &[
                serde_json::json!({"type": "error", "error": {"type": "server_error",
                "code": "internal_server_error", "message": "upstream fell over near fake-secret"}}),
            ],
            Some(&credential),
        );
        let nested = nested.unwrap_err().to_string();
        assert!(nested.contains("type server_error"), "{nested}");
        assert!(nested.contains("upstream fell over"), "{nested}");
        assert!(!nested.contains("type error"), "{nested}");
        assert!(!nested.contains("fake-secret"), "{nested}");

        let (flat, _) = assemble(
            &[serde_json::json!({"type": "error", "code": "server_error", "message": "try again"})],
            None,
        );
        let flat = flat.unwrap_err().to_string();
        assert!(flat.contains("code server_error: try again"), "{flat}");
        assert!(!flat.contains("type error"), "{flat}");

        let (refused, _) = assemble(
            &[
                serde_json::json!({"type": "error", "error": {"type": "rate_limit_error",
                "message": "usage limit reached"}}),
            ],
            None,
        );
        assert!(matches!(
            refused.unwrap_err(),
            BackendError::Refused {
                cause: RefusalCause::UsageLimit,
                ..
            }
        ));
    }

    #[test]
    fn text_the_deltas_did_not_deliver_is_taken_from_the_completed_item() {
        let message_done = |index: u64, text: &str| {
            serde_json::json!({"type": "response.output_item.done", "output_index": index,
                "item": {"type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": text}]}})
        };
        let completed = serde_json::json!({"type": "response.completed", "response": {}});

        // A gateway that sends no deltas at all.
        let (response, streamed) =
            assemble(&[message_done(0, "Whole reply."), completed.clone()], None);
        assert_eq!(response.unwrap().text, "Whole reply.");
        assert_eq!(streamed, "Whole reply.");

        // Deltas that stop short, then a second message item: the first is
        // completed from its item, and the second is set off from it.
        let (response, streamed) = assemble(
            &[
                serde_json::json!({"type": "response.output_text.delta", "output_index": 0,
                    "delta": "Looking at "}),
                message_done(0, "Looking at the flange."),
                call_done(1, "call_1", "{}"),
                serde_json::json!({"type": "response.output_text.delta", "output_index": 2,
                    "delta": "Then the bore."}),
                message_done(2, "Then the bore."),
                completed,
            ],
            None,
        );
        let response = response.unwrap();
        assert_eq!(response.text, "Looking at the flange.\n\nThen the bore.");
        assert_eq!(
            streamed, response.text,
            "the user read what the model is shown"
        );
    }

    #[tokio::test]
    async fn a_text_request_cut_off_at_the_output_limit_is_an_error() {
        let (base_url, _, server) = recording_server(vec![ScriptedResponse {
            status: 200,
            body: events(&[
                serde_json::json!({"type": "response.output_text.delta", "delta": "partial"}),
                serde_json::json!({"type": "response.incomplete", "response": {
                    "incomplete_details": {"reason": "max_output_tokens"}}}),
            ]),
            streamed: true,
            delay: None,
        }])
        .await;
        let error = client(&base_url, None)
            .request_text("i", "x", CancelFlag::new())
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(error.to_string().contains("output limit"), "{error}");
    }

    #[test]
    fn a_context_window_is_read_under_any_of_the_spellings_endpoints_use() {
        let read = |record: serde_json::Value| advertised_context_window(&record);
        assert_eq!(
            read(serde_json::json!({"context_window": 200000})),
            Some(200_000)
        );
        assert_eq!(
            read(serde_json::json!({"context_length": 131072})),
            Some(131_072)
        );
        assert_eq!(
            read(serde_json::json!({"top_provider": {"context_length": 1048576}})),
            Some(1_048_576)
        );
        assert_eq!(
            read(serde_json::json!({"meta": {"n_ctx_train": 32768}})),
            Some(32_768)
        );
        assert_eq!(
            read(serde_json::json!({"model_info": {"llama.context_length": 8192}})),
            Some(8_192)
        );
        assert_eq!(
            read(serde_json::json!({"max_input_tokens": 128000})),
            Some(128_000)
        );
        assert_eq!(
            read(serde_json::json!({"id": "m", "object": "model"})),
            None
        );
        assert_eq!(
            read(serde_json::json!({"context_window": 12})),
            None,
            "an implausibly small figure is a mistake, not a window"
        );
    }

    #[tokio::test]
    async fn the_context_window_is_asked_of_the_model_record_then_the_list() {
        // The model's own record answers.
        let (base_url, records, server) = recording_server(vec![ScriptedResponse {
            status: 200,
            body: serde_json::json!({"id": "configured-model", "context_window": 200000})
                .to_string(),
            streamed: false,
            delay: None,
        }])
        .await;
        let probe = client(&base_url, Some("fake-token"));
        assert_eq!(probe.context_window(CancelFlag::new()).await, Some(200_000));
        server.await.unwrap();
        {
            let records = records.lock().unwrap();
            assert_eq!(records[0].path, "/v1/models/configured-model");
            assert!(records[0].authenticated);
        }

        // The record says nothing; the list carries it.
        let (base_url, records, server) = recording_server(vec![
            ScriptedResponse {
                status: 404,
                body: "{}".to_string(),
                streamed: false,
                delay: None,
            },
            ScriptedResponse {
                status: 200,
                body: serde_json::json!({"data": [
                    {"id": "other", "context_length": 4096},
                    {"id": "configured-model", "context_length": 131072},
                ]})
                .to_string(),
                streamed: false,
                delay: None,
            },
        ])
        .await;
        let probe = client(&base_url, None);
        assert_eq!(probe.context_window(CancelFlag::new()).await, Some(131_072));
        server.await.unwrap();
        assert_eq!(records.lock().unwrap()[1].path, "/v1/models");

        // Nothing advertised anywhere: not an error.
        let (base_url, _records, server) = recording_server(vec![
            ScriptedResponse {
                status: 200,
                body: serde_json::json!({"id": "configured-model"}).to_string(),
                streamed: false,
                delay: None,
            },
            ScriptedResponse {
                status: 200,
                body: serde_json::json!({"data": [{"id": "configured-model"}]}).to_string(),
                streamed: false,
                delay: None,
            },
        ])
        .await;
        let probe = client(&base_url, None);
        assert_eq!(probe.context_window(CancelFlag::new()).await, None);
        server.await.unwrap();
    }

    #[test]
    fn a_malformed_event_between_valid_ones_fails_the_stream_without_echoing_it() {
        let mut parser = EventParser::default();
        let stream = format!(
            "data: {}\n\ndata: {{\"type\": \"b\", secret-token\n\ndata: {}\n\n",
            serde_json::json!({"type": "a"}),
            serde_json::json!({"type": "c"})
        );
        let error = parser.push(stream.as_bytes()).unwrap_err();
        let BackendError::ParseError(detail) = &error else {
            panic!("expected a parse error, got {error:?}");
        };
        assert!(detail.contains("malformed stream event"), "{detail}");
        assert!(!detail.contains("secret-token"), "{detail}");
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
            refusal_cause(
                StatusCode::BAD_GATEWAY,
                &error("model_cooldown", "usage limit reached")
            ),
            Some(RefusalCause::UsageLimit)
        );
        assert_eq!(
            refusal_cause(StatusCode::SERVICE_UNAVAILABLE, &error("", "")),
            Some(RefusalCause::Overloaded)
        );
        assert_eq!(
            refusal_cause(
                StatusCode::from_u16(529).unwrap(),
                &error("overloaded_error", "Overloaded")
            ),
            Some(RefusalCause::Overloaded)
        );
        assert_eq!(
            refusal_cause(
                StatusCode::INTERNAL_SERVER_ERROR,
                &error("", "server is busy, no slots available")
            ),
            Some(RefusalCause::Overloaded)
        );
        assert_eq!(
            refusal_cause(StatusCode::UNAUTHORIZED, &error("", "")),
            Some(RefusalCause::Authentication)
        );
        assert_eq!(
            refusal_cause(
                StatusCode::NOT_FOUND,
                &error("model_not_found", "no such model")
            ),
            Some(RefusalCause::UnknownModel)
        );
        assert_eq!(
            refusal_cause(
                StatusCode::INTERNAL_SERVER_ERROR,
                &error("boom", "exploded")
            ),
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
            tool_call(
                "run_script",
                serde_json::json!({"code": "x = 1", "summary": "Set x"}),
            ),
        ])
        .await;
        let client = client(&base_url, Some("fake-token"));

        let deltas = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&deltas);
        let mut sink = move |delta: StreamDelta| recorded.lock().unwrap().push(delta);
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
        assert_eq!(
            *deltas.lock().unwrap(),
            [StreamDelta::Text("distinctive".into())]
        );

        let second = client
            .stream(request, CancelFlag::new(), &mut sink)
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(second.tool_calls.len(), 1);
        assert_eq!(second.tool_calls[0].name, "run_script");
        assert_eq!(second.tool_calls[0].arguments["code"], "x = 1");
        assert!(matches!(
            deltas.lock().unwrap().last(),
            Some(StreamDelta::ToolCallStarted { name }) if name == "run_script"
        ));

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
        assert!(
            body["input"][0]["content"][1]["image_url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert_eq!(body["input"][1]["role"], "assistant");
        assert_eq!(body["input"][2]["type"], "function_call");
        assert_eq!(body["input"][2]["call_id"], "call_0");
        assert_eq!(body["input"][3]["type"], "function_call_output");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "run_script");
        assert_eq!(
            body["tools"].as_array().unwrap().len(),
            crate::tools::tools_for(true).len()
        );
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
            .request_text("instructions", "input", CancelFlag::new())
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
    async fn a_stream_that_never_completes_is_a_parse_error_and_a_failed_response_is_read_by_cause()
    {
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
            client.request_text("i", "x", CancelFlag::new()).await,
            Err(BackendError::ParseError(_))
        ));
        let failed = client
            .request_text("i", "x", CancelFlag::new())
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(matches!(
            failed,
            BackendError::Refused {
                cause: RefusalCause::Overloaded,
                ..
            }
        ));
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
            client
                .request_text("instructions", "input", CancelFlag::new())
                .await,
            Err(BackendError::Unavailable(_))
        ));
    }
}
