// One AI turn as an agentic loop. The model is asked, it answers with text
// or tool calls, the tools run (write and execute the script; look up
// documentation; look at the render), their results go back, and the loop
// continues until the model answers without a tool call or the user
// cancels. Every step is reported as an event so the chat pane can show
// what is happening and the viewport can rebuild after each execution.
//
// The loop is written over traits — the model, the script executor, the
// render source — so it is proven against scripted stand-ins; the
// orchestrator supplies the production pieces.
//
// What a turn leaves behind: on completion, the last script that executed
// successfully is on disk and the model it built is on screen; on failure
// or cancellation, the script the turn started from is back on disk and
// the caller restores the model it had.

use std::path::{Path, PathBuf};

use cadmark_bridge::SYSTEM_PROMPT;
use cadmark_bridge::backend::{
    BackendError, ImageData, ModelItem, ModelRequest, StreamDelta, ToolCall, TurnModel,
};
use cadmark_bridge::examples;
use cadmark_bridge::grounding::{GroundedComment, render_comment};
use cadmark_bridge::tools::{
    LOOKUP_DOCS, LookupDocsArgs, RENDER_VIEW, RUN_SCRIPT, RenderView, RenderViewArgs,
    RunScriptArgs, tools_for, unavailable_tools_note,
};
use cadmark_core::cancellation::CancelFlag;
use cadmark_core::mesh::TessellatedMesh;
use cadmark_core::message::{ContextUsage, Conversation, MessageKind};
use cadmark_kernel::protocol::ExecutedModel;
use cadmark_kernel::worker::WorkerError;

use crate::validity::describe_validity;

/// What the user sent to start a turn: any chat text, the pending
/// comments with their anchors, and the project's reference images.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnInput {
    pub chat: Option<String>,
    pub comments: Vec<GroundedComment>,
    pub images: Vec<ImageData>,
    /// The configured provider context window. Compatible providers do not
    /// expose one shared capability, so this comes from user settings.
    pub context_window_tokens: usize,
}

impl Default for TurnInput {
    fn default() -> Self {
        Self {
            chat: None,
            comments: Vec::new(),
            images: Vec::new(),
            context_window_tokens: crate::user_settings::DEFAULT_CONTEXT_WINDOW_TOKENS,
        }
    }
}

/// Something the turn did, reported as it happens.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    /// The step the turn is on, for the phase line.
    Phase(String),
    /// More of the model's reply text.
    Text(String),
    /// A tool call began.
    ToolStarted {
        call_id: String,
        tool: String,
        arguments: serde_json::Value,
    },
    /// A tool call ended with this output.
    ToolFinished {
        call_id: String,
        output: String,
        failed: bool,
    },
    /// A script executed successfully mid-turn; the viewport shows it.
    ModelBuilt {
        model: Box<ExecutedModel>,
        source: String,
    },
    /// The earlier conversation has been replaced by this model-written
    /// account before the next request could approach its context limit.
    ConversationCondensed { summary: String },
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    /// At least one execution succeeded; the last good script is on disk
    /// and its model on screen, and this is the design step's summary.
    Completed {
        summary: String,
        model: Box<ExecutedModel>,
        source: String,
    },
    /// The model answered without changing the script (a question, a
    /// refusal, an explanation). Nothing to commit.
    Answered,
    /// The turn ended with no successful execution after trying: the
    /// original script is back on disk and the caller restores its model.
    Failed { error: String },
    /// The user cancelled; the original script is back on disk.
    Cancelled,
}

/// Runs scripts. The kernel worker in production; scripted outcomes in
/// tests.
pub trait ScriptExecutor: Send {
    fn execute(
        &mut self,
        script_path: &Path,
        cancel: &CancelFlag,
    ) -> Result<ExecutedModel, WorkerError>;
}

/// Answers the documentation tool.
pub trait DocSource: Send + Sync {
    /// Answer `query`, giving up when `cancel` is set.
    fn lookup(
        &self,
        query: &str,
        cancel: CancelFlag,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + '_>>;
}

/// Produces the render the model asked for. The viewport owns the GPU, so
/// in production this hands the request to the UI thread and waits.
pub trait RenderSource: Send {
    fn render(&mut self, view: RenderView) -> Result<ImageData, String>;

    /// The model an in-turn execution just produced. The viewport puts
    /// the same mesh on screen, but only once the UI thread next runs a
    /// frame; a render asked for in the same response as the execution
    /// would otherwise be of the previous model, or of nothing at all.
    fn model_built(&mut self, _mesh: &TessellatedMesh) {}
}

/// A render source for a model that cannot see: the tool is not offered,
/// and a call to it anyway is answered honestly.
pub struct NoRender;

impl RenderSource for NoRender {
    fn render(&mut self, _view: RenderView) -> Result<ImageData, String> {
        Err("rendering is not available to this model".to_string())
    }
}

/// Everything one turn needs.
pub struct TurnRunner<
    'a,
    M: TurnModel + ?Sized,
    E: ScriptExecutor,
    D: DocSource + ?Sized,
    R: RenderSource + ?Sized,
> {
    pub model: &'a M,
    pub executor: &'a mut E,
    pub docs: &'a D,
    pub render: &'a mut R,
    pub script_path: PathBuf,
    pub cancel: CancelFlag,
}

impl<M: TurnModel + ?Sized, E: ScriptExecutor, D: DocSource + ?Sized, R: RenderSource + ?Sized>
    TurnRunner<'_, M, E, D, R>
{
    /// Run one turn. `conversation` is the history the model is shown;
    /// `input` is what starts this turn. Events are delivered to `emit`
    /// as they happen.
    pub async fn run(
        &mut self,
        conversation: &Conversation,
        input: &TurnInput,
        mut emit: impl FnMut(TurnEvent) + Send,
    ) -> TurnOutcome {
        let original = std::fs::read_to_string(&self.script_path).ok();
        let mut items = history_items(conversation);
        let usage = context_usage(
            conversation,
            input.images.len(),
            input.context_window_tokens,
        );
        if usage.needs_condensing() && !conversation.is_empty() {
            let summary = match self.condense(conversation).await {
                Ok(summary) => summary,
                Err(outcome) => return outcome,
            };
            emit(TurnEvent::ConversationCondensed {
                summary: summary.clone(),
            });
            items = vec![ModelItem::Assistant {
                text: format!("Conversation summary:\n{summary}"),
            }];
        }
        let request = render_input(input);
        // The curated example library for the operations this request
        // names, carried before the user's words so the request itself
        // stays last.
        items.push(ModelItem::User {
            text: examples::context_block(&request),
            images: Vec::new(),
        });
        items.push(ModelItem::User {
            text: request,
            images: input.images.clone(),
        });
        let tools = tools_for(self.model.accepts_images());
        let instructions = instructions_for(self.model.accepts_images());
        let mut last_good: Option<(String, Box<ExecutedModel>, String)> = None;
        let mut last_failure: Option<String> = None;
        let mut attempt = 0u32;

        loop {
            if self.cancel.is_cancelled() {
                return self.abort(original.as_deref(), TurnOutcome::Cancelled);
            }
            emit(TurnEvent::Phase("thinking".to_string()));
            let request = ModelRequest {
                instructions: instructions.clone(),
                items: items.clone(),
                tools: tools.clone(),
            };
            let mut sink = |delta: StreamDelta| match delta {
                StreamDelta::Text(text) => emit(TurnEvent::Text(text)),
                StreamDelta::ToolCallStarted { name } => {
                    emit(TurnEvent::Phase(format!(
                        "preparing to {}",
                        describe_tool(&name)
                    )));
                }
            };
            let response = match self
                .model
                .respond(request, self.cancel.clone(), &mut sink)
                .await
            {
                Ok(response) => response,
                Err(BackendError::Cancelled) => {
                    return self.abort(original.as_deref(), TurnOutcome::Cancelled);
                }
                Err(error) => {
                    return self.abort(
                        original.as_deref(),
                        TurnOutcome::Failed {
                            error: error.to_string(),
                        },
                    );
                }
            };

            if !response.text.is_empty() {
                items.push(ModelItem::Assistant {
                    text: response.text.clone(),
                });
            }
            if response.tool_calls.is_empty() {
                break;
            }

            for call in response.tool_calls {
                if self.cancel.is_cancelled() {
                    return self.abort(original.as_deref(), TurnOutcome::Cancelled);
                }
                emit(TurnEvent::ToolStarted {
                    call_id: call.id.clone(),
                    tool: call.name.clone(),
                    arguments: call.arguments.clone(),
                });
                let mut rendered = None;
                let (output, failed) = match self.run_tool(&call, &mut attempt, &mut emit).await {
                    ToolRun::Output { output, failed } => (output, failed),
                    ToolRun::Rendered { image } => {
                        rendered = Some(image);
                        ("rendered; the image follows".to_string(), false)
                    }
                    ToolRun::Built {
                        code,
                        model,
                        summary,
                    } => {
                        // Before the event: the UI thread shows this mesh
                        // a frame later, and a render may be asked for in
                        // this same response.
                        self.render.model_built(&model.mesh);
                        emit(TurnEvent::ModelBuilt {
                            model: model.clone(),
                            source: code.clone(),
                        });
                        let output = describe_model(&model);
                        last_good = Some((code, model, summary));
                        last_failure = None;
                        (output, false)
                    }
                    ToolRun::Abort(outcome) => {
                        return self.abort(original.as_deref(), outcome);
                    }
                };
                if failed && call.name == RUN_SCRIPT {
                    last_failure = Some(output.clone());
                }
                emit(TurnEvent::ToolFinished {
                    call_id: call.id.clone(),
                    output: output.clone(),
                    failed,
                });
                items.push(ModelItem::ToolCall(call.clone()));
                items.push(ModelItem::ToolResult {
                    call_id: call.id,
                    output,
                });
                if let Some(image) = rendered {
                    // A tool result carries text only on the wire; the
                    // image rides as the next user item.
                    items.push(ModelItem::User {
                        text: format!(
                            "The render you asked for ({}).",
                            describe_view(render_view_of(&call.arguments))
                        ),
                        images: vec![image],
                    });
                }
            }
        }

        match last_good {
            Some((code, model, summary)) => {
                // A later attempt may have failed after the last success;
                // the script on disk must be the one the model on screen
                // came from.
                if let Err(error) = std::fs::write(&self.script_path, &code) {
                    return TurnOutcome::Failed {
                        error: format!("could not write the script: {error}"),
                    };
                }
                TurnOutcome::Completed {
                    summary,
                    model,
                    source: code,
                }
            }
            None => match last_failure {
                Some(error) => self.abort(original.as_deref(), TurnOutcome::Failed { error }),
                None => TurnOutcome::Answered,
            },
        }
    }

    /// Ask the same configured model to retain the information that makes a
    /// resumed project useful before old detail uses the available context.
    async fn condense(&self, conversation: &Conversation) -> Result<String, TurnOutcome> {
        let request = ModelRequest {
            instructions: "Condense this earlier CAD conversation for the next turn. Retain every decision already made and every request still open, including dimensions, constraints, and unresolved questions. Drop only chatter and completed detail. Return the compact account alone; do not call tools.".to_string(),
            items: history_items(conversation),
            tools: Vec::new(),
        };
        let mut sink = |_delta: StreamDelta| {};
        match self.model.respond(request, self.cancel.clone(), &mut sink).await {
            Ok(response) if !response.tool_calls.is_empty() => Err(TurnOutcome::Failed {
                error: "The AI tried to use a tool while condensing the conversation; the earlier conversation was kept."
                    .to_string(),
            }),
            Ok(response) if response.text.trim().is_empty() => Err(TurnOutcome::Failed {
                error: "The AI returned an empty conversation summary; the earlier conversation was kept."
                    .to_string(),
            }),
            Ok(response) => Ok(response.text),
            Err(BackendError::Cancelled) => Err(TurnOutcome::Cancelled),
            Err(error) => Err(TurnOutcome::Failed {
                error: format!("Could not condense the conversation: {error}"),
            }),
        }
    }

    async fn run_tool(
        &mut self,
        call: &ToolCall,
        attempt: &mut u32,
        emit: &mut (impl FnMut(TurnEvent) + Send),
    ) -> ToolRun {
        match call.name.as_str() {
            RUN_SCRIPT => {
                let args: RunScriptArgs = match serde_json::from_value(call.arguments.clone()) {
                    Ok(args) => args,
                    Err(error) => return ToolRun::bad_arguments(error),
                };
                *attempt += 1;
                emit(TurnEvent::Phase(format!(
                    "running the script, attempt {attempt}"
                )));
                if let Err(error) = std::fs::write(&self.script_path, &args.code) {
                    return ToolRun::Abort(TurnOutcome::Failed {
                        error: format!("could not write the script: {error}"),
                    });
                }
                match self.executor.execute(&self.script_path, &self.cancel) {
                    Ok(model) => ToolRun::Built {
                        code: args.code,
                        model: Box::new(model),
                        summary: args.summary,
                    },
                    Err(error) if error.is_script_fault() => ToolRun::Output {
                        output: format!("The script failed:\n{error}"),
                        failed: true,
                    },
                    Err(WorkerError::Cancelled) => ToolRun::Abort(TurnOutcome::Cancelled),
                    Err(error) => ToolRun::Abort(TurnOutcome::Failed {
                        error: error.to_string(),
                    }),
                }
            }
            LOOKUP_DOCS => {
                let args: LookupDocsArgs = match serde_json::from_value(call.arguments.clone()) {
                    Ok(args) => args,
                    Err(error) => return ToolRun::bad_arguments(error),
                };
                emit(TurnEvent::Phase("looking up build123d docs".to_string()));
                let answer = self.docs.lookup(&args.query, self.cancel.clone()).await;
                if self.cancel.is_cancelled() {
                    return ToolRun::Abort(TurnOutcome::Cancelled);
                }
                ToolRun::Output {
                    output: answer,
                    failed: false,
                }
            }
            RENDER_VIEW => {
                let args: RenderViewArgs = match serde_json::from_value(call.arguments.clone()) {
                    Ok(args) => args,
                    Err(error) => return ToolRun::bad_arguments(error),
                };
                emit(TurnEvent::Phase(format!(
                    "looking at the render from the {}",
                    describe_view(args.view)
                )));
                match self.render.render(args.view) {
                    Ok(image) => ToolRun::Rendered { image },
                    Err(reason) => ToolRun::Output {
                        output: format!("render unavailable: {reason}"),
                        failed: true,
                    },
                }
            }
            other => ToolRun::Output {
                output: format!("unknown tool {other}"),
                failed: true,
            },
        }
    }

    /// Put the original script back (or remove one that did not exist)
    /// and return the outcome.
    fn abort(&self, original: Option<&str>, outcome: TurnOutcome) -> TurnOutcome {
        let restored = match original {
            Some(code) => std::fs::write(&self.script_path, code),
            None => match std::fs::remove_file(&self.script_path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            },
        };
        match restored {
            Ok(()) => outcome,
            Err(error) => TurnOutcome::Failed {
                error: format!(
                    "{}; and the original script could not be restored: {error}",
                    match &outcome {
                        TurnOutcome::Failed { error } => error.clone(),
                        TurnOutcome::Cancelled => "the turn was cancelled".to_string(),
                        _ => "the turn ended".to_string(),
                    }
                ),
            },
        }
    }
}

enum ToolRun {
    Output {
        output: String,
        failed: bool,
    },
    Built {
        code: String,
        model: Box<ExecutedModel>,
        summary: String,
    },
    /// A render: the tool result is text, and the image itself follows
    /// as a user item so the model can see it.
    Rendered {
        image: ImageData,
    },
    Abort(TurnOutcome),
}

impl ToolRun {
    fn bad_arguments(error: serde_json::Error) -> Self {
        Self::Output {
            output: format!("the tool arguments were not valid: {error}"),
            failed: true,
        }
    }
}

/// The turn's opening message: chat text and every comment with its anchors.
fn render_input(input: &TurnInput) -> String {
    let mut parts: Vec<String> = input.comments.iter().map(render_comment).collect();
    if let Some(chat) = &input.chat {
        parts.push(chat.clone());
    }
    parts.join("\n\n")
}

/// The conversation so far as the model sees it.
fn history_items(conversation: &Conversation) -> Vec<ModelItem> {
    let mut items = Vec::new();
    for message in conversation.messages() {
        match &message.kind {
            MessageKind::UserChat => items.push(ModelItem::User {
                text: message.text.clone(),
                images: Vec::new(),
            }),
            MessageKind::SpatialComment { anchors, .. } => items.push(ModelItem::User {
                text: render_comment(&GroundedComment {
                    text: message.text.clone(),
                    anchors: anchors.clone(),
                }),
                images: Vec::new(),
            }),
            MessageKind::AiResponse => {
                if !message.text.is_empty() {
                    items.push(ModelItem::Assistant {
                        text: message.text.clone(),
                    });
                }
            }
            MessageKind::ConversationSummary => items.push(ModelItem::Assistant {
                text: format!("Conversation summary:\n{}", message.text),
            }),
            MessageKind::ToolCalls(activities) => {
                for activity in activities {
                    items.push(ModelItem::ToolCall(ToolCall {
                        id: activity.call_id.clone(),
                        name: activity.tool.clone(),
                        arguments: activity.arguments.clone(),
                    }));
                    items.push(ModelItem::ToolResult {
                        call_id: activity.call_id.clone(),
                        output: activity.output.clone().unwrap_or_default(),
                    });
                }
            }
            MessageKind::Notice { .. } => {}
        }
    }
    items
}

/// The accounting shown to the user and used to start condensation. Text is
/// estimated provider-neutrally; images reserve a conservative fixed budget
/// until the image attachment work can supply model-specific measurements.
pub const REFERENCE_IMAGE_TOKENS: usize = 765;

pub fn context_usage(
    conversation: &Conversation,
    reference_image_count: usize,
    window_tokens: usize,
) -> ContextUsage {
    ContextUsage {
        conversation_tokens: conversation.estimated_tokens(),
        reference_image_tokens: reference_image_count * REFERENCE_IMAGE_TOKENS,
        window_tokens,
    }
}

/// What the model reads after a successful execution.
fn describe_model(model: &ExecutedModel) -> String {
    let mut text = format!(
        "Executed successfully. Model: {}.",
        model.summary.describe()
    );
    text.push(' ');
    text.push_str(&describe_validity(&model.validity));
    let untraced = model.ledger.untraced_count();
    if untraced > 0 {
        text.push_str(&format!(
            " {untraced} elements came from operations CADmark cannot trace."
        ));
    }
    text
}

fn describe_tool(name: &str) -> &str {
    match name {
        RUN_SCRIPT => "run the script",
        LOOKUP_DOCS => "look up the docs",
        RENDER_VIEW => "look at the render",
        other => other,
    }
}

/// The view a render call named, for the caption; a call whose arguments
/// did not parse never reaches here.
/// The instructions one turn is run with: the system prompt, plus what
/// the model is owed about any tool it is not being offered.
fn instructions_for(accepts_images: bool) -> String {
    match unavailable_tools_note(accepts_images) {
        Some(note) => format!("{SYSTEM_PROMPT}\n\n{note}"),
        None => SYSTEM_PROMPT.to_string(),
    }
}

fn render_view_of(arguments: &serde_json::Value) -> RenderView {
    serde_json::from_value::<RenderViewArgs>(arguments.clone())
        .map(|args| args.view)
        .unwrap_or(RenderView::Current)
}

fn describe_view(view: RenderView) -> &'static str {
    match view {
        RenderView::Current => "current camera",
        RenderView::Front => "front",
        RenderView::Back => "back",
        RenderView::Left => "left",
        RenderView::Right => "right",
        RenderView::Top => "top",
        RenderView::Bottom => "bottom",
        RenderView::Isometric => "isometric view",
    }
}

/// Reference images the model reads with every turn: one per file in the
/// project folder's `references/` directory.
pub fn reference_images(project_dir: &Path) -> Vec<ImageData> {
    let mut images = Vec::new();
    let Ok(entries) = std::fs::read_dir(project_dir.join("references")) else {
        return images;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        let Some(media_type) = reference_image_media_type(&path) else {
            continue;
        };
        if let Ok(bytes) = std::fs::read(&path) {
            images.push(ImageData {
                media_type: media_type.to_string(),
                bytes,
            });
        }
    }
    images
}

/// Count reference images without opening them. The frame loop needs this for
/// occupancy display, while a turn alone pays to load their bytes.
pub fn reference_image_count(project_dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(project_dir.join("references")) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| reference_image_media_type(&entry.path()).is_some())
        .count()
}

fn reference_image_media_type(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use crate::render_source::{RenderGpu, SceneHandle, ViewportRender};
    use cadmark_bridge::backend::{DeltaSink, ModelResponse};
    use cadmark_core::geometry::{GeometryDescriptors, ModelSummary, SolidValidity};
    use cadmark_core::ledger::ProvenanceLedger;
    use cadmark_core::limits::{ExecutionLimits, LimitHit};
    use cadmark_core::mesh::TessellatedMesh;
    use cadmark_core::message::Message;
    use cadmark_kernel::protocol::ModelFile;

    /// A model that answers from a script of responses and records what
    /// it was asked.
    struct ScriptedModel {
        responses: Mutex<VecDeque<Result<ModelResponse, BackendError>>>,
        requests: Mutex<Vec<ModelRequest>>,
        accepts_images: bool,
    }

    impl ScriptedModel {
        fn new(responses: impl IntoIterator<Item = Result<ModelResponse, BackendError>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                requests: Mutex::new(Vec::new()),
                accepts_images: false,
            }
        }
    }

    impl TurnModel for ScriptedModel {
        fn model_name(&self) -> &str {
            "scripted"
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
            self.requests.lock().unwrap().push(request);
            let next = self.responses.lock().unwrap().pop_front();
            Box::pin(async move {
                if cancel.is_cancelled() {
                    return Err(BackendError::Cancelled);
                }
                let response = next.unwrap_or_else(|| Ok(ModelResponse::default()))?;
                if !response.text.is_empty() {
                    sink(StreamDelta::Text(response.text.clone()));
                }
                Ok(response)
            })
        }
    }

    /// A summary stand-in whose output is derived from what the loop actually
    /// sent. It makes the retention assertion fail if either commitment never
    /// reaches the condensation request, rather than seeding the answer in a
    /// canned fixture.
    struct RetainingSummaryModel {
        requests: Mutex<Vec<ModelRequest>>,
    }

    impl RetainingSummaryModel {
        fn new() -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl TurnModel for RetainingSummaryModel {
        fn model_name(&self) -> &str {
            "retaining-summary"
        }

        fn accepts_images(&self) -> bool {
            false
        }

        fn respond<'a>(
            &'a self,
            request: ModelRequest,
            _cancel: CancelFlag,
            _sink: DeltaSink<'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<ModelResponse, BackendError>> + Send + 'a>,
        > {
            let mut requests = self.requests.lock().unwrap();
            let response = if requests.is_empty() {
                let transcript = request
                    .items
                    .iter()
                    .map(|item| match item {
                        ModelItem::User { text, .. } | ModelItem::Assistant { text } => text,
                        ModelItem::ToolCall(_) | ModelItem::ToolResult { .. } => "",
                    })
                    .collect::<String>();
                if transcript.contains("Decision: use a 5 mm wall")
                    && transcript.contains("Open request: add a lid")
                {
                    text("Decision: use a 5 mm wall. Open request: add a lid after review.")
                } else {
                    text("Conversation summary was incomplete.")
                }
            } else {
                text("I will continue from the summary.")
            };
            requests.push(request);
            Box::pin(async move { response })
        }
    }

    fn text(reply: &str) -> Result<ModelResponse, BackendError> {
        Ok(ModelResponse {
            text: reply.to_string(),
            tool_calls: Vec::new(),
        })
    }

    fn run_script(id: &str, code: &str, summary: &str) -> Result<ModelResponse, BackendError> {
        Ok(ModelResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: RUN_SCRIPT.to_string(),
                arguments: serde_json::json!({"code": code, "summary": summary}),
            }],
        })
    }

    fn lookup(id: &str, query: &str) -> Result<ModelResponse, BackendError> {
        Ok(ModelResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: LOOKUP_DOCS.to_string(),
                arguments: serde_json::json!({"query": query}),
            }],
        })
    }

    /// An executor whose outcomes are scripted and whose executed code is
    /// recorded.
    #[derive(Clone)]
    struct FakeExecutor {
        outcomes: Arc<Mutex<VecDeque<Result<(), WorkerError>>>>,
        executed: Arc<Mutex<Vec<String>>>,
        block_restoration: bool,
    }

    impl FakeExecutor {
        fn new(outcomes: impl IntoIterator<Item = Result<(), WorkerError>>) -> Self {
            Self {
                outcomes: Arc::new(Mutex::new(outcomes.into_iter().collect())),
                executed: Arc::new(Mutex::new(Vec::new())),
                block_restoration: false,
            }
        }

        fn blocking_restoration(
            outcomes: impl IntoIterator<Item = Result<(), WorkerError>>,
        ) -> Self {
            let mut executor = Self::new(outcomes);
            executor.block_restoration = true;
            executor
        }

        fn executed(&self) -> Vec<String> {
            self.executed.lock().unwrap().clone()
        }
    }

    fn sample_model() -> ExecutedModel {
        ExecutedModel {
            mesh: TessellatedMesh::default(),
            ledger: ProvenanceLedger::new(),
            descriptors: GeometryDescriptors::default(),
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
            model: ModelFile(PathBuf::from("/scratch/model-1.brep")),
        }
    }

    impl ScriptExecutor for FakeExecutor {
        fn execute(
            &mut self,
            script_path: &Path,
            _cancel: &CancelFlag,
        ) -> Result<ExecutedModel, WorkerError> {
            let code = std::fs::read_to_string(script_path).unwrap();
            self.executed.lock().unwrap().push(code);
            let outcome = self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(()));
            if self.block_restoration {
                std::fs::remove_file(script_path).unwrap();
                std::fs::create_dir(script_path).unwrap();
            }
            outcome.map(|()| sample_model())
        }
    }

    struct FakeRender;

    impl RenderSource for FakeRender {
        fn render(&mut self, _view: RenderView) -> Result<ImageData, String> {
            Ok(ImageData {
                media_type: "image/png".into(),
                bytes: vec![1, 2, 3],
            })
        }
    }

    /// An executor whose every run produces a model with real geometry,
    /// for the tests that then render what was built.
    struct CubeExecutor;

    impl ScriptExecutor for CubeExecutor {
        fn execute(
            &mut self,
            _script_path: &Path,
            _cancel: &CancelFlag,
        ) -> Result<ExecutedModel, WorkerError> {
            Ok(ExecutedModel {
                mesh: cube_mesh(),
                ..sample_model()
            })
        }
    }

    struct FakeDocs;

    impl DocSource for FakeDocs {
        fn lookup(
            &self,
            query: &str,
            _cancel: CancelFlag,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + '_>> {
            let answer = format!("DOCS FOR {query}");
            Box::pin(async move { answer })
        }
    }

    struct Harness {
        /// Held so the project folder outlives the harness.
        _project: tempfile::TempDir,
        script: PathBuf,
        executor: FakeExecutor,
        events: Vec<TurnEvent>,
    }

    impl Harness {
        fn with_script(original: Option<&str>, executor: FakeExecutor) -> Self {
            let project = tempfile::tempdir().unwrap();
            let script = project.path().join("part.py");
            if let Some(original) = original {
                std::fs::write(&script, original).unwrap();
            }
            Self {
                _project: project,
                script,
                executor,
                events: Vec::new(),
            }
        }

        async fn run(
            &mut self,
            model: &ScriptedModel,
            input: TurnInput,
            cancel: CancelFlag,
        ) -> TurnOutcome {
            let mut render = NoRender;
            let mut runner = TurnRunner {
                model,
                executor: &mut self.executor,
                docs: &FakeDocs,
                render: &mut render,
                script_path: self.script.clone(),
                cancel,
            };
            let events = &mut self.events;
            runner
                .run(&Conversation::new(), &input, |event| events.push(event))
                .await
        }

        async fn run_with_conversation<M: TurnModel>(
            &mut self,
            model: &M,
            conversation: &Conversation,
            input: TurnInput,
            cancel: CancelFlag,
        ) -> TurnOutcome {
            let mut render = NoRender;
            let mut runner = TurnRunner {
                model,
                executor: &mut self.executor,
                docs: &FakeDocs,
                render: &mut render,
                script_path: self.script.clone(),
                cancel,
            };
            let events = &mut self.events;
            runner
                .run(conversation, &input, |event| events.push(event))
                .await
        }

        fn on_disk(&self) -> Option<String> {
            std::fs::read_to_string(&self.script).ok()
        }

        fn phases(&self) -> Vec<&str> {
            self.events
                .iter()
                .filter_map(|event| match event {
                    TurnEvent::Phase(phase) => Some(phase.as_str()),
                    _ => None,
                })
                .collect()
        }
    }

    /// The whole text the model was shown on its first request.
    fn first_request_context(model: &ScriptedModel) -> String {
        model.requests.lock().unwrap()[0]
            .items
            .iter()
            .map(|item| match item {
                ModelItem::User { text, .. } => text.clone(),
                ModelItem::Assistant { text } => text.clone(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn the_request_carries_example_material_for_the_operation_it_names() {
        let model = ScriptedModel::new([text("Revolved it.")]);
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([]));
        harness
            .run(
                &model,
                chat("revolve this section about the axis"),
                CancelFlag::new(),
            )
            .await;
        let context = first_request_context(&model);
        // The revolve example's own script, not merely a non-empty block.
        assert!(
            context.contains("revolve(axis=Axis.Z"),
            "no revolve example reached the model"
        );
        // The discriminating half: an operation this request did not name
        // brings no material of its own.
        assert!(
            !context.contains("chamfer("),
            "an unrelated example reached the model"
        );

        let other = ScriptedModel::new([text("Rounded it.")]);
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([]));
        harness
            .run(&other, chat("fillet these corners"), CancelFlag::new())
            .await;
        let context = first_request_context(&other);
        assert!(
            context.contains("chamfer("),
            "no fillet example reached the model"
        );
        assert!(
            !context.contains("revolve(axis=Axis.Z"),
            "an unrelated example reached the model"
        );
    }

    #[tokio::test]
    async fn a_request_naming_several_operations_carries_material_for_each() {
        let model = ScriptedModel::new([text("Done.")]);
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([]));
        harness
            .run(
                &model,
                chat("extrude the profile, revolve the boss, cut a hole, then fillet the corners"),
                CancelFlag::new(),
            )
            .await;
        let context = first_request_context(&model);
        // One marker from each of the four examples the request names;
        // dropping any operation's material fails here.
        for (operation, marker) in [
            ("extrude", "extrude(amount=plate_thickness)"),
            ("revolve", "revolve(axis=Axis.Z"),
            ("cut", "Hole(radius="),
            ("fillet", "chamfer("),
        ] {
            assert!(
                context.contains(marker),
                "no {operation} example reached the model"
            );
        }
    }

    #[tokio::test]
    async fn a_spatial_comment_selects_examples_from_the_words_it_carries() {
        let model = ScriptedModel::new([text("Bored it.")]);
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([]));
        harness
            .run(
                &model,
                TurnInput {
                    chat: None,
                    comments: vec![GroundedComment {
                        text: "bore a hole through here".into(),
                        anchors: Vec::new(),
                    }],
                    images: Vec::new(),
                    context_window_tokens: crate::user_settings::DEFAULT_CONTEXT_WINDOW_TOKENS,
                },
                CancelFlag::new(),
            )
            .await;
        let context = first_request_context(&model);
        assert!(
            context.contains("Hole(radius="),
            "no cut example reached the model"
        );
    }

    fn chat(text: &str) -> TurnInput {
        TurnInput {
            chat: Some(text.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn context_usage_includes_reference_images_found_in_the_project() {
        let project = tempfile::tempdir().unwrap();
        let references = project.path().join("references");
        std::fs::create_dir(&references).unwrap();
        std::fs::write(
            references.join("bracket.png"),
            "image bytes are not opened for counting",
        )
        .unwrap();
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("x".repeat(300)));

        let without_images = context_usage(&conversation, 0, 1_000);
        let with_images =
            context_usage(&conversation, reference_image_count(project.path()), 1_000);

        assert_eq!(with_images.conversation_tokens, 75);
        assert_eq!(with_images.reference_image_tokens, REFERENCE_IMAGE_TOKENS);
        assert_eq!(
            with_images.used_tokens(),
            without_images.used_tokens() + REFERENCE_IMAGE_TOKENS
        );
        assert!(with_images.needs_condensing());
    }

    #[tokio::test]
    async fn condensation_shows_the_next_request_a_summary_with_decisions_and_open_requests() {
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat(format!(
            "Decision: use a 5 mm wall. {}",
            "detail ".repeat(700)
        )));
        conversation.push(Message::ai_response("The wall will be 5 mm."));
        conversation.push(Message::user_chat(
            "Open request: add a lid after the next review.",
        ));
        let model = RetainingSummaryModel::new();
        let mut harness = Harness::with_script(Some("original"), FakeExecutor::new([]));

        let outcome = harness
            .run_with_conversation(
                &model,
                &conversation,
                TurnInput {
                    chat: Some("What remains?".into()),
                    context_window_tokens: 1_000,
                    ..Default::default()
                },
                CancelFlag::new(),
            )
            .await;

        assert_eq!(outcome, TurnOutcome::Answered);
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].items.iter().any(
            |item| matches!(item, ModelItem::User { text, .. } if text.contains("5 mm wall"))
        ));
        let summary_shown = requests[1].items.iter().find_map(|item| match item {
            ModelItem::Assistant { text } => Some(text),
            _ => None,
        });
        assert!(summary_shown.is_some_and(|text| text.contains("5 mm wall")));
        assert!(summary_shown.is_some_and(|text| text.contains("add a lid")));
        assert!(matches!(
            harness.events.first(),
            Some(TurnEvent::ConversationCondensed { summary }) if summary.contains("5 mm wall")
        ));
    }

    #[tokio::test]
    async fn a_turn_iterates_through_docs_failure_and_success_then_commits_the_last_good_script() {
        let model = ScriptedModel::new([
            lookup("c1", "fillet"),
            run_script("c2", "BAD = 1", "First try"),
            run_script("c3", "GOOD = 1", "Fixed"),
            text("Done: a box."),
        ]);
        let mut harness = Harness::with_script(
            Some("ORIGINAL = 1"),
            FakeExecutor::new([Err(WorkerError::Script("NameError: BAD".into())), Ok(())]),
        );
        let outcome = harness
            .run(&model, chat("make a box"), CancelFlag::new())
            .await;

        assert!(matches!(
            &outcome,
            TurnOutcome::Completed { summary, source, .. } if summary == "Fixed" && source == "GOOD = 1"
        ));
        assert_eq!(harness.on_disk().as_deref(), Some("GOOD = 1"));
        assert_eq!(harness.executor.executed(), ["BAD = 1", "GOOD = 1"]);
        assert_eq!(
            harness.phases(),
            [
                "thinking",
                "looking up build123d docs",
                "thinking",
                "running the script, attempt 1",
                "thinking",
                "running the script, attempt 2",
                "thinking",
            ]
        );
        assert!(harness.events.iter().any(|event| matches!(
            event,
            TurnEvent::ToolFinished { call_id, output, failed: true } if call_id == "c2" && output.contains("NameError")
        )));
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| matches!(event, TurnEvent::ModelBuilt { .. }))
                .count(),
            1
        );
        assert!(
            harness
                .events
                .contains(&TurnEvent::Text("Done: a box.".into()))
        );

        // Every tool result went back to the model paired with its call,
        // and the docs answer reached it.
        let requests = model.requests.lock().unwrap();
        let last = requests.last().unwrap();
        assert!(last.items.iter().any(|item| matches!(
            item,
            ModelItem::ToolResult { call_id, output } if call_id == "c1" && output == "DOCS FOR fillet"
        )));
        assert!(last.items.iter().any(|item| matches!(
            item,
            ModelItem::ToolResult { call_id, output } if call_id == "c3" && output.contains("closed and valid")
        )));
        assert_eq!(last.tools.len(), 2, "no render tool for a text-only model");
    }

    #[tokio::test]
    async fn a_turn_that_never_executes_successfully_restores_the_original_and_reports_the_error() {
        let model = ScriptedModel::new([
            run_script("c1", "BAD = 1", "Try"),
            text("I could not get this to work."),
        ]);
        let mut harness = Harness::with_script(
            Some("ORIGINAL = 1"),
            FakeExecutor::new([Err(WorkerError::Script("SyntaxError".into()))]),
        );
        let outcome = harness.run(&model, chat("do it"), CancelFlag::new()).await;
        assert!(matches!(&outcome, TurnOutcome::Failed { error } if error.contains("SyntaxError")));
        assert_eq!(harness.on_disk().as_deref(), Some("ORIGINAL = 1"));
    }

    #[tokio::test]
    async fn syntax_errors_and_tracebacks_are_shown_to_the_model_for_correction() {
        let model = ScriptedModel::new([
            run_script("syntax", "BROKEN =", "Syntax attempt"),
            run_script("traceback", "missing_name()", "Runtime attempt"),
            run_script("fixed", "GOOD = 1", "Fixed"),
            text("Fixed both errors."),
        ]);
        let mut harness = Harness::with_script(
            Some("ORIGINAL = 1"),
            FakeExecutor::new([
                Err(WorkerError::Script("SyntaxError: invalid syntax".into())),
                Err(WorkerError::Script(
                    "Traceback (most recent call last): NameError: missing_name".into(),
                )),
                Ok(()),
            ]),
        );

        let outcome = harness
            .run(&model, chat("make it work"), CancelFlag::new())
            .await;

        assert!(matches!(outcome, TurnOutcome::Completed { .. }));
        assert_eq!(
            harness.executor.executed(),
            ["BROKEN =", "missing_name()", "GOOD = 1"]
        );
        let requests = model.requests.lock().unwrap();
        assert!(requests[1].items.iter().any(|item| matches!(
            item,
            ModelItem::ToolResult { call_id, output }
                if call_id == "syntax" && output.contains("SyntaxError: invalid syntax")
        )));
        assert!(requests[2].items.iter().any(|item| matches!(
            item,
            ModelItem::ToolResult { call_id, output }
                if call_id == "traceback" && output.contains("Traceback") && output.contains("missing_name")
        )));
    }

    #[tokio::test]
    async fn a_failed_restoration_is_reported_in_the_turn_outcome() {
        let model = ScriptedModel::new([
            run_script("c1", "BAD = 1", "Try"),
            text("I could not get this to work."),
        ]);
        let mut harness = Harness::with_script(
            Some("ORIGINAL = 1"),
            FakeExecutor::blocking_restoration([Err(WorkerError::Script("SyntaxError".into()))]),
        );

        let outcome = harness.run(&model, chat("do it"), CancelFlag::new()).await;

        assert!(matches!(
            outcome,
            TurnOutcome::Failed { error }
                if error.contains("SyntaxError") && error.contains("could not be restored")
        ));
    }

    #[tokio::test]
    async fn a_limit_hit_is_handed_back_to_the_model_as_a_script_fault() {
        let model = ScriptedModel::new([
            run_script("c1", "while True: pass", "Loop"),
            run_script("c2", "FIXED = 1", "Fixed"),
            text("Fixed the loop."),
        ]);
        let limits = ExecutionLimits::default();
        let mut harness = Harness::with_script(
            None,
            FakeExecutor::new([
                Err(WorkerError::Limit {
                    hit: LimitHit::WallClock,
                    limits,
                }),
                Ok(()),
            ]),
        );
        let outcome = harness.run(&model, chat("go"), CancelFlag::new()).await;
        assert!(matches!(outcome, TurnOutcome::Completed { .. }));
        let requests = model.requests.lock().unwrap();
        assert!(requests[1].items.iter().any(|item| matches!(
            item,
            ModelItem::ToolResult { call_id, output } if call_id == "c1" && output.contains("wall-clock limit")
        )));
    }

    #[tokio::test]
    async fn a_runtime_fault_ends_the_turn_and_is_not_handed_to_the_model() {
        let model = ScriptedModel::new([run_script("c1", "X = 1", "Try"), text("unreached")]);
        let mut harness = Harness::with_script(
            Some("ORIGINAL = 1"),
            FakeExecutor::new([Err(WorkerError::Runtime("worker died".into()))]),
        );
        let outcome = harness.run(&model, chat("go"), CancelFlag::new()).await;
        assert!(matches!(&outcome, TurnOutcome::Failed { error } if error.contains("worker died")));
        assert_eq!(harness.on_disk().as_deref(), Some("ORIGINAL = 1"));
        assert_eq!(
            model.requests.lock().unwrap().len(),
            1,
            "the model was not asked again"
        );
    }

    #[tokio::test]
    async fn cancelling_mid_turn_restores_the_original_script() {
        let cancel = CancelFlag::new();
        let model = ScriptedModel::new([run_script("c1", "HALF = 1", "Half"), text("unreached")]);
        let mut harness = Harness::with_script(
            Some("ORIGINAL = 1"),
            FakeExecutor::new([Err(WorkerError::Cancelled)]),
        );
        let outcome = harness.run(&model, chat("go"), cancel).await;
        assert_eq!(outcome, TurnOutcome::Cancelled);
        assert_eq!(harness.on_disk().as_deref(), Some("ORIGINAL = 1"));
    }

    #[tokio::test]
    async fn a_turn_on_a_new_project_that_fails_leaves_no_script_behind() {
        let model = ScriptedModel::new([run_script("c1", "BAD = 1", "Try"), text("gave up")]);
        let mut harness = Harness::with_script(
            None,
            FakeExecutor::new([Err(WorkerError::Script("boom".into()))]),
        );
        let outcome = harness.run(&model, chat("go"), CancelFlag::new()).await;
        assert!(matches!(outcome, TurnOutcome::Failed { .. }));
        assert_eq!(harness.on_disk(), None);
    }

    #[tokio::test]
    async fn an_answer_without_tool_calls_changes_nothing() {
        let model = ScriptedModel::new([text("A fillet rounds an edge.")]);
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([]));
        let outcome = harness
            .run(&model, chat("what is a fillet?"), CancelFlag::new())
            .await;
        assert_eq!(outcome, TurnOutcome::Answered);
        assert_eq!(harness.on_disk().as_deref(), Some("ORIGINAL = 1"));
        assert!(harness.executor.executed().is_empty());
    }

    #[tokio::test]
    async fn a_provider_refusal_ends_the_turn_by_cause() {
        let model = ScriptedModel::new([Err(BackendError::Refused {
            cause: cadmark_bridge::backend::RefusalCause::UsageLimit,
            detail: "HTTP 429".into(),
        })]);
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([]));
        let outcome = harness.run(&model, chat("go"), CancelFlag::new()).await;
        assert!(matches!(&outcome, TurnOutcome::Failed { error } if error.contains("usage limit")));
    }

    #[tokio::test]
    async fn every_comment_anchor_reaches_the_model_and_the_last_good_script_wins() {
        use cadmark_core::geometry::{EdgeId, FaceId, GeometryContext, TopologyElement};
        use cadmark_core::ledger::LedgerValue;
        let anchor = |element: TopologyElement| GeometryContext {
            sketch: Default::default(),
            element,
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
        };
        let input = TurnInput {
            chat: Some("and make it taller".into()),
            comments: vec![
                GroundedComment {
                    text: "round this".into(),
                    anchors: vec![
                        anchor(TopologyElement::Face(FaceId(3))),
                        anchor(TopologyElement::Edge(EdgeId(4))),
                    ],
                },
                GroundedComment {
                    text: "and chamfer this".into(),
                    anchors: vec![anchor(TopologyElement::Edge(EdgeId(9)))],
                },
            ],
            images: Vec::new(),
            ..Default::default()
        };
        let model = ScriptedModel::new([
            run_script("c1", "GOOD = 1", "Rounded"),
            run_script("c2", "WORSE = 1", "Taller"),
            text("The taller version failed; keeping the rounded one."),
        ]);
        let mut harness = Harness::with_script(
            Some("ORIGINAL = 1"),
            FakeExecutor::new([Ok(()), Err(WorkerError::Script("boom".into()))]),
        );
        let outcome = harness.run(&model, input, CancelFlag::new()).await;
        assert!(matches!(&outcome, TurnOutcome::Completed { source, .. } if source == "GOOD = 1"));
        assert_eq!(harness.on_disk().as_deref(), Some("GOOD = 1"));

        let requests = model.requests.lock().unwrap();
        let ModelItem::User { text, .. } = &requests[0].items[1] else {
            panic!("the user's turn follows the example library");
        };
        for expected in [
            "- face 3",
            "- edge 4",
            "- edge 9",
            "round this",
            "and chamfer this",
            "and make it taller",
        ] {
            assert!(text.contains(expected), "missing {expected}");
        }
    }

    #[tokio::test]
    async fn a_render_reaches_the_model_as_an_image_on_the_next_request() {
        let render_call = Ok(ModelResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: RENDER_VIEW.into(),
                arguments: serde_json::json!({"view": "top"}),
            }],
        });
        let mut model = ScriptedModel::new([render_call, text("Looks right.")]);
        model.accepts_images = true;
        let project = tempfile::tempdir().unwrap();
        let script = project.path().join("part.py");
        let mut executor = FakeExecutor::new([]);
        let mut render = FakeRender;
        let mut runner = TurnRunner {
            model: &model,
            executor: &mut executor,
            docs: &FakeDocs,
            render: &mut render,
            script_path: script,
            cancel: CancelFlag::new(),
        };
        let outcome = runner
            .run(&Conversation::new(), &chat("check it"), |_event| {})
            .await;
        assert_eq!(outcome, TurnOutcome::Answered);
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests[0].tools.len(), 3, "the render tool is offered");
        let second = &requests[1];
        assert!(
            matches!(&second.items[3], ModelItem::ToolResult { call_id, .. } if call_id == "c1")
        );
        assert!(matches!(
            &second.items[4],
            ModelItem::User { text, images } if text.contains("top") && images.len() == 1
        ));
    }

    /// A unit cube standing in for a model: enough geometry to fill a
    /// framed render, with the normals the shading pass needs.
    fn cube_mesh() -> cadmark_core::mesh::TessellatedMesh {
        use cadmark_core::mesh::{MeshEdge, MeshVertex};

        let corners = [
            [-0.5f32, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5],
        ];
        let faces: [([usize; 4], [f32; 3]); 6] = [
            ([0, 3, 2, 1], [0.0, 0.0, -1.0]),
            ([4, 5, 6, 7], [0.0, 0.0, 1.0]),
            ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
            ([2, 3, 7, 6], [0.0, 1.0, 0.0]),
            ([1, 2, 6, 5], [1.0, 0.0, 0.0]),
            ([3, 0, 4, 7], [-1.0, 0.0, 0.0]),
        ];

        let mut mesh = cadmark_core::mesh::TessellatedMesh::default();
        for (face_id, (quad, normal)) in faces.iter().enumerate() {
            let base = mesh.vertices.len() as u32;
            for &corner in quad {
                mesh.vertices.push(MeshVertex {
                    position: corners[corner],
                    normal: *normal,
                });
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            mesh.face_ids
                .extend_from_slice(&[face_id as u32, face_id as u32]);
        }
        for (edge_id, (a, b)) in [(0, 1), (1, 2), (2, 3), (3, 0)].iter().enumerate() {
            mesh.edges.push(MeshEdge {
                points: vec![corners[*a], corners[*b]],
                edge_id: edge_id as u32,
            });
        }
        mesh
    }

    /// A device for the rendering tests: the real adapter where there is
    /// one, the software fallback otherwise. A runner with neither fails
    /// the test rather than passing without rendering anything.
    async fn gpu() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::default();
        let mut found = None;
        for force_fallback_adapter in [false, true] {
            found = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter,
                })
                .await;
            if found.is_some() {
                break;
            }
        }
        let adapter = found
            .expect("no GPU adapter and no software fallback: install a Vulkan ICD or lavapipe");
        adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .expect("the adapter gave no device")
    }

    #[tokio::test]
    async fn the_model_is_shown_a_real_render_of_what_is_on_screen() {
        let (device, queue) = gpu().await;
        let scene = SceneHandle::new();
        scene.set_mesh(Some((
            std::sync::Arc::new(cube_mesh()),
            Some(cadmark_renderer::camera::Bounds3 {
                min: [-0.5, -0.5, -0.5],
                max: [0.5, 0.5, 0.5],
            }),
        )));
        scene.set_view(cadmark_renderer::camera::Camera::default(), (800, 600));
        let mut render = ViewportRender::new(scene, RenderGpu { device, queue });

        let render_call = Ok(ModelResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: RENDER_VIEW.into(),
                arguments: serde_json::json!({"view": "front"}),
            }],
        });
        let mut model = ScriptedModel::new([render_call, text("Looks right.")]);
        model.accepts_images = true;
        let project = tempfile::tempdir().unwrap();
        let mut executor = FakeExecutor::new([]);
        let mut runner = TurnRunner {
            model: &model,
            executor: &mut executor,
            docs: &FakeDocs,
            render: &mut render,
            script_path: project.path().join("part.py"),
            cancel: CancelFlag::new(),
        };
        let mut events = Vec::new();
        let outcome = runner
            .run(&Conversation::new(), &chat("check it"), |event| {
                events.push(event)
            })
            .await;
        assert_eq!(outcome, TurnOutcome::Answered);

        // The user is told what the turn is doing while it looks.
        assert!(
            events.iter().any(|event| matches!(
                event,
                TurnEvent::Phase(phase) if phase.contains("looking at the render") && phase.contains("front")
            )),
            "the render phase is announced, got {events:?}"
        );

        let requests = model.requests.lock().unwrap();
        // The image is joined to the call it answers: the tool result for
        // c1, then the image item that follows it.
        let result_at = requests[1]
            .items
            .iter()
            .position(
                |item| matches!(item, ModelItem::ToolResult { call_id, .. } if call_id == "c1"),
            )
            .expect("the render call is answered");
        let ModelItem::User { text, images } = &requests[1].items[result_at + 1] else {
            panic!("the render is shown as a user item straight after its result");
        };
        assert!(text.contains("front"));
        let image = images.first().expect("an image item, not an apology");
        assert_eq!(image.media_type, "image/png");

        // The pixels are the viewport's own, and the model is framed to
        // fill them rather than sitting in a corner: neither is reachable
        // from the stand-in source, which returns an error and no image.
        let decoder = png::Decoder::new(std::io::Cursor::new(&image.bytes));
        let mut reader = decoder.read_info().expect("PNG header");
        let mut pixels = vec![0; reader.output_buffer_size().expect("PNG buffer size")];
        let info = reader.next_frame(&mut pixels).expect("PNG data");
        assert_eq!((info.width, info.height), (800, 600));
        let total = (info.width * info.height) as f32;
        let background = [36u8, 38, 43];
        let covered = pixels[..info.buffer_size()]
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| {
                [p[0], p[1], p[2]]
                    .iter()
                    .zip(background)
                    .any(|(got, base)| got.abs_diff(base) > 8)
            })
            .count() as f32;
        assert!(
            covered / total > 0.2,
            "the model fills the frame, covered {:.3}",
            covered / total
        );
    }

    #[tokio::test]
    async fn a_render_asked_for_beside_the_execution_shows_the_model_just_built() {
        // Nothing has ever been published into the scene: the UI thread
        // has not run a frame since the script executed, which is the
        // normal case when the model asks to run and to look in one
        // response.
        let (device, queue) = gpu().await;
        let scene = SceneHandle::new();
        scene.set_view(cadmark_renderer::camera::Camera::default(), (400, 300));
        let mut render = ViewportRender::new(scene, RenderGpu { device, queue });

        let build_and_look = Ok(ModelResponse {
            text: String::new(),
            tool_calls: vec![
                ToolCall {
                    id: "c1".into(),
                    name: RUN_SCRIPT.into(),
                    arguments: serde_json::json!({"code": "X = 1", "summary": "Box"}),
                },
                ToolCall {
                    id: "c2".into(),
                    name: RENDER_VIEW.into(),
                    arguments: serde_json::json!({"view": "front"}),
                },
            ],
        });
        let mut model = ScriptedModel::new([build_and_look, text("Looks right.")]);
        model.accepts_images = true;
        let project = tempfile::tempdir().unwrap();
        let mut executor = CubeExecutor;
        let mut runner = TurnRunner {
            model: &model,
            executor: &mut executor,
            docs: &FakeDocs,
            render: &mut render,
            script_path: project.path().join("part.py"),
            cancel: CancelFlag::new(),
        };
        let outcome = runner
            .run(
                &Conversation::new(),
                &chat("build a box and check it"),
                |_e| {},
            )
            .await;
        assert!(matches!(outcome, TurnOutcome::Completed { .. }));

        let requests = model.requests.lock().unwrap();
        let result_at = requests[1]
            .items
            .iter()
            .position(
                |item| matches!(item, ModelItem::ToolResult { call_id, .. } if call_id == "c2"),
            )
            .expect("the render call is answered");
        let ModelItem::User { images, .. } = &requests[1].items[result_at + 1] else {
            panic!("the render is shown as a user item, not an apology that nothing is on screen");
        };
        let image = images.first().expect("an image of the model just built");
        let decoder = png::Decoder::new(std::io::Cursor::new(&image.bytes));
        let mut reader = decoder.read_info().expect("PNG header");
        let mut pixels = vec![0; reader.output_buffer_size().expect("PNG buffer size")];
        let info = reader.next_frame(&mut pixels).expect("PNG data");
        assert_eq!((info.width, info.height), (400, 300));
        let background = [36u8, 38, 43];
        let covered = pixels[..info.buffer_size()]
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| {
                [p[0], p[1], p[2]]
                    .iter()
                    .zip(background)
                    .any(|(got, base)| got.abs_diff(base) > 8)
            })
            .count() as f32;
        assert!(
            covered / (info.width * info.height) as f32 > 0.2,
            "the freshly built model fills the frame"
        );
    }

    #[test]
    fn a_text_only_model_is_told_the_render_tool_is_unavailable() {
        let blind = instructions_for(false);
        assert!(blind.starts_with(SYSTEM_PROMPT));
        assert!(blind.contains(RENDER_VIEW) && blind.contains("unavailable"));
        assert_eq!(instructions_for(true), SYSTEM_PROMPT);
    }

    #[test]
    fn history_replays_tool_calls_paired_with_their_results() {
        use cadmark_core::message::{Message, ToolActivity};
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("make a box"));
        conversation.push(Message::tool_calls(vec![ToolActivity {
            call_id: "c1".into(),
            tool: RUN_SCRIPT.into(),
            arguments: serde_json::json!({"code": "X = 1", "summary": "Box"}),
            output: Some("Executed successfully.".into()),
            failed: false,
            started: chrono::Utc::now(),
            finished: None,
        }]));
        conversation.push(Message::ai_response("Made a box."));
        conversation.push(Message::error_notice("not for the model"));
        let items = history_items(&conversation);
        assert_eq!(items.len(), 4);
        assert!(matches!(&items[1], ModelItem::ToolCall(call) if call.id == "c1"));
        assert!(matches!(&items[2], ModelItem::ToolResult { call_id, .. } if call_id == "c1"));
        assert!(matches!(&items[3], ModelItem::Assistant { text } if text == "Made a box."));
    }
}
