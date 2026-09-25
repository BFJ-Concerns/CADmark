// One AI turn as an agentic loop. The model is asked, it answers with text
// or tool calls, the tools run (edit, read and execute the script; run a
// scratch snippet; look up documentation; look at the render), their
// results go back, and the loop continues until the model answers without
// a tool call or the user cancels. Every step is reported as an event so the chat pane can show
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
    BackendError, ImageData, ModelItem, ModelRequest, ProviderUsage, RequestPurpose, StreamDelta,
    ToolCall, ToolSpec, TurnModel,
};
use cadmark_bridge::examples;
use cadmark_bridge::grounding::{GroundedComment, render_comment};
use cadmark_bridge::tools::{
    EDIT_SCRIPT, EditScriptArgs, KEEP_REFERENCE, KeepReferenceArgs, KeepReferenceSource,
    LOOKUP_DOCS, LookupDocsArgs, READ_SCRIPT, REFERENCE_IMAGES, RENDER_VIEW, RUN_PYTHON,
    RUN_SCRIPT, ReadScriptArgs, ReferenceImagesArgs, RenderView, RenderViewArgs, RunPythonArgs,
    RunScriptArgs, tools_for, unavailable_tools_note,
};
use cadmark_core::cancellation::CancelFlag;
use cadmark_core::geometry::describe_parts;
use cadmark_core::message::{
    ContextUsage, Conversation, IMAGE_TOKENS, ImageAttachment, Message, MessageKind,
    estimate_tokens,
};
use cadmark_core::skills;
use cadmark_kernel::protocol::{ExecutedModel, ModelForm, SnippetOutcome};
use cadmark_kernel::worker::WorkerError;

use crate::script_parameters;
use crate::validity::describe_validity;

/// What the user sent to start a turn: any chat text, the pending
/// comments with their anchors, and the images attached to them.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnInput {
    pub chat: Option<String>,
    pub comments: Vec<GroundedComment>,
    /// The images attached to this turn's messages. Sent to the model
    /// only when it reads images; the stored file name is what
    /// `keep_reference` refers to them by.
    pub images: Vec<ImageAttachment>,
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
    /// A tool call ended with this output. `executed` is the script text
    /// a successful `run_script` ran, for the conversation's record.
    ToolFinished {
        call_id: String,
        output: String,
        failed: bool,
        executed: Option<String>,
    },
    /// A script executed successfully mid-turn; the viewport shows it.
    ModelBuilt {
        model: Box<ExecutedModel>,
        source: String,
    },
    /// The earlier conversation has been replaced by this model-written
    /// account before the next request could approach its context limit.
    ConversationCondensed { summary: String },
    /// Something the user should know about how the turn is going, said
    /// by CADmark rather than the AI.
    Notice(String),
    /// What the provider reported one request of the turn cost.
    Usage(ProviderUsage),
    /// The item sequence the turn ended with: every request's items and
    /// the reply that closed it. Sent last, however the turn ended, so
    /// the next turn's request extends this sequence.
    ModelContext {
        items: Vec<ModelItem>,
        identity: Option<String>,
    },
    /// The model is reasoning before it answers: a piece of the reasoning
    /// text where the provider shares it, or empty where it does not, so
    /// a long think shows as work rather than silence.
    Thinking(String),
}

/// Shown in the chat when the provider cuts a reply off at its output
/// limit and the turn carries on.
const OUTPUT_LIMIT_NOTICE: &str = "The AI's reply reached the provider's output limit and was \
    cut off there; CADmark kept what it had written and asked it to continue.";

/// Told to the model after a reply the provider cut off at its output
/// limit.
const CONTINUE_AFTER_OUTPUT_LIMIT: &str = "Your last response reached the provider's output \
    limit and was cut off. Its text, retained reasoning and complete tool calls are above; a tool \
    call you had not finished writing was dropped and did not run. Continue from where you \
    stopped, making any dropped call again in full.";

/// The turn's error when a reply is cut off before it holds anything to
/// continue from.
const OUTPUT_LIMIT_WITHOUT_PROGRESS: &str = "The AI's reply reached the provider's output limit \
    (max_output_tokens) without text, a complete tool call or resumable reasoning, so there was \
    nothing to continue from. A higher output limit at the provider or gateway, or a smaller \
    request, may get through.";

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    /// At least one execution succeeded; the last good script is on disk
    /// and its model on screen, and this is the design step's summary.
    Completed {
        summary: String,
        model: Box<ExecutedModel>,
        source: String,
        /// What became of each locked parameter the turn moved, unlocked,
        /// or removed, for the user to see in chat after the AI's closing
        /// message whatever that message says about it.
        locked_changes: Vec<String>,
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

    /// Run a scratch snippet under the execution ceilings: after the
    /// script at `script_path` when one is given, alone otherwise.
    fn run_snippet(
        &mut self,
        script_path: Option<&Path>,
        code: &str,
        cancel: &CancelFlag,
    ) -> Result<SnippetOutcome, WorkerError>;
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
    /// The model from `view`: every part the user has visible, or the one
    /// part `part` names alone.
    fn render(&mut self, view: RenderView, part: Option<&str>) -> Result<ImageData, String>;

    /// The model an in-turn execution just produced, solid or sketch.
    /// The viewport puts the same geometry on screen, but only once the
    /// UI thread next runs a frame; a render asked for in the same
    /// response as the execution would otherwise be of the previous
    /// model, or of nothing at all.
    fn model_built(&mut self, _model: &ExecutedModel) {}
}

/// A render source for a model that cannot see: the tool is not offered,
/// and a call to it anyway is answered honestly.
pub struct NoRender;

impl RenderSource for NoRender {
    fn render(&mut self, _view: RenderView, _part: Option<&str>) -> Result<ImageData, String> {
        Err("rendering is not available to this model".to_string())
    }
}

/// A file in the reference library as the listing tool reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceListing {
    pub file: String,
    /// The index line for the file, when the AI has written one.
    pub description: Option<String>,
}

/// Answers the reference-library tools: the project's `references/`
/// folder and its index in production, a scripted stand-in in tests.
pub trait ReferenceSource: Send {
    fn list(&self) -> Result<Vec<ReferenceListing>, String>;
    fn read(&self, file: &str) -> Result<ImageData, String>;
    /// Keep `image` under a name based on `file`, described in the index.
    /// Returns the file name used.
    fn keep(&self, image: &ImageData, file: &str, description: &str) -> Result<String, String>;
    /// Replace the index line for a file already in the library.
    fn describe(&self, file: &str, description: &str) -> Result<(), String>;
}

/// A reference source for a model that cannot see: the tools are not
/// offered, and a call to them anyway is answered honestly.
pub struct NoReferences;

impl ReferenceSource for NoReferences {
    fn list(&self) -> Result<Vec<ReferenceListing>, String> {
        Err("the reference library is not available to this model".to_string())
    }

    fn read(&self, _file: &str) -> Result<ImageData, String> {
        Err("the reference library is not available to this model".to_string())
    }

    fn keep(&self, _image: &ImageData, _file: &str, _description: &str) -> Result<String, String> {
        Err("the reference library is not available to this model".to_string())
    }

    fn describe(&self, _file: &str, _description: &str) -> Result<(), String> {
        Err("the reference library is not available to this model".to_string())
    }
}

/// Everything one turn needs.
pub struct TurnRunner<
    'a,
    M: TurnModel + ?Sized,
    E: ScriptExecutor,
    D: DocSource + ?Sized,
    R: RenderSource + ?Sized,
    L: ReferenceSource + ?Sized,
> {
    pub model: &'a M,
    pub executor: &'a mut E,
    pub docs: &'a D,
    pub render: &'a mut R,
    pub references: &'a L,
    pub script_path: PathBuf,
    pub cancel: CancelFlag,
}

impl<
    M: TurnModel + ?Sized,
    E: ScriptExecutor,
    D: DocSource + ?Sized,
    R: RenderSource + ?Sized,
    L: ReferenceSource + ?Sized,
> TurnRunner<'_, M, E, D, R, L>
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
        let identity = self.model.session_identity();
        let conversation = conversation.for_model(identity.as_deref());
        let mut items = Vec::new();
        let outcome = self
            .run_with(&conversation, input, &mut emit, &mut items)
            .await;
        settle_unanswered_calls(&mut items);
        emit(TurnEvent::ModelContext { items, identity });
        outcome
    }

    /// The turn itself. `items` is the conversation as the model is shown
    /// it, left holding the sequence the turn ended with.
    async fn run_with(
        &mut self,
        conversation: &Conversation,
        input: &TurnInput,
        mut emit: impl FnMut(TurnEvent) + Send,
        items: &mut Vec<ModelItem>,
    ) -> TurnOutcome {
        let original = std::fs::read_to_string(&self.script_path).ok();
        // Every image the conversation holds, newest first, so a request
        // to keep "the drawing" finds the one from three turns ago as
        // readily as this turn's.
        let attachments: Vec<ImageAttachment> = input
            .images
            .iter()
            .cloned()
            .chain(
                conversation
                    .messages()
                    .iter()
                    .rev()
                    .flat_map(|message| message.attachments.iter().cloned()),
            )
            .collect();
        let assembled = RequestAssembly::new(
            conversation,
            original.as_deref(),
            input,
            self.model.accepts_images(),
        );
        let usage = assembled.usage(input.context_window_tokens);
        // The recorded session first, exactly as the model last saw it,
        // then whatever the chat gained after it.
        let (recorded, unrendered) = conversation.replay();
        *items = recorded.to_vec();
        items.extend(history_items(unrendered, self.model.accepts_images()));
        // A condensation that fails ends the turn, but only after the
        // turn's own input has joined the items: the sequence the turn
        // ends with is recorded as the next request's prefix, so the
        // request the user just made must be in it.
        let condensation_failure = if usage.needs_condensing() && !conversation.is_empty() {
            match self.condense(conversation).await {
                Ok(summary) => {
                    emit(TurnEvent::ConversationCondensed {
                        summary: summary.clone(),
                    });
                    *items = vec![ModelItem::Assistant {
                        text: format!("Conversation summary:\n{summary}"),
                    }];
                    None
                }
                Err(outcome) => Some(outcome),
            }
        } else {
            None
        };
        items.extend(assembled.items);
        if let Some(outcome) = condensation_failure {
            return outcome;
        }
        let tools = assembled.tools;
        let instructions = assembled.instructions;
        let mut last_good: Option<(String, Box<ExecutedModel>, String)> = None;
        let mut last_failure: Option<String> = None;
        let mut attempt = 0u32;

        loop {
            if self.cancel.is_cancelled() {
                return self.abort(original.as_deref(), TurnOutcome::Cancelled);
            }
            emit(TurnEvent::Phase("thinking".to_string()));
            let request = ModelRequest {
                purpose: RequestPurpose::Turn,
                instructions: instructions.clone(),
                items: items.clone(),
                tools: tools.clone(),
            };
            let mut sink = |delta: StreamDelta| match delta {
                StreamDelta::Text(text) => emit(TurnEvent::Text(text)),
                StreamDelta::Reasoning(text) => emit(TurnEvent::Thinking(text)),
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

            if let Some(usage) = response.usage {
                emit(TurnEvent::Usage(usage));
            }
            items.extend(response.replay_items());
            // A reply cut off at the output limit is kept, its whole tool
            // calls run, and the model is asked to go on; one cut off
            // before it wrote anything would only be cut off again.
            let cut_off = response.reached_output_limit;
            if cut_off
                && response.text.trim().is_empty()
                && response.tool_calls.is_empty()
                && !response.has_continuable_reasoning()
            {
                return self.abort(
                    original.as_deref(),
                    TurnOutcome::Failed {
                        error: OUTPUT_LIMIT_WITHOUT_PROGRESS.to_string(),
                    },
                );
            }
            if response.tool_calls.is_empty() && !cut_off {
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
                let mut executed = None;
                let (output, failed) = match self
                    .run_tool(&call, &mut attempt, &attachments, &mut emit)
                    .await
                {
                    ToolRun::Output { output, failed } => (output, failed),
                    ToolRun::Rendered { image } => {
                        rendered = Some(image);
                        ("rendered; the image follows".to_string(), false)
                    }
                    ToolRun::ImageRead { file, image } => {
                        rendered = Some(image);
                        (format!("{file}; the image follows"), false)
                    }
                    ToolRun::Built {
                        code,
                        model,
                        summary,
                    } => {
                        // Before the event: the UI thread shows this mesh
                        // a frame later, and a render may be asked for in
                        // this same response.
                        self.render.model_built(&model);
                        emit(TurnEvent::ModelBuilt {
                            model: model.clone(),
                            source: code.clone(),
                        });
                        let mut output = describe_model(&model);
                        // A locked parameter the run moved is put to the
                        // model at once, while it can still restore it.
                        let moved = locked_parameter_changes(original.as_deref(), &code);
                        if !moved.is_empty() {
                            output.push_str(&format!(
                                "\n\nLocked parameters: {}. A locked parameter changes only with \
                                 the user's explicit permission in this conversation. Without \
                                 it, restore the value (keeping the `# locked` marker) and run \
                                 again; with it, say so in your final message.",
                                moved.join("; ")
                            ));
                        }
                        executed = Some(code.clone());
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
                    executed,
                });
                items.push(ModelItem::ToolResult {
                    call_id: call.id,
                    output,
                });
                if let Some(image) = rendered {
                    // A tool result carries text only on the wire; the
                    // image rides as the next user item.
                    items.push(ModelItem::User {
                        text: if call.name == REFERENCE_IMAGES {
                            "The reference image you asked for.".to_string()
                        } else {
                            format!(
                                "The render you asked for, {}.",
                                describe_render(&render_args_of(&call.arguments))
                            )
                        },
                        images: vec![image],
                    });
                }
            }
            if cut_off {
                emit(TurnEvent::Notice(OUTPUT_LIMIT_NOTICE.to_string()));
                items.push(ModelItem::User {
                    text: CONTINUE_AFTER_OUTPUT_LIMIT.to_string(),
                    images: Vec::new(),
                });
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
                    locked_changes: locked_parameter_changes(original.as_deref(), &code),
                    source: code,
                }
            }
            None => match last_failure {
                Some(error) => self.abort(original.as_deref(), TurnOutcome::Failed { error }),
                // Edits with no successful run behind them are not a
                // result: the file goes back to how the turn found it.
                None if std::fs::read_to_string(&self.script_path).ok() != original => self.abort(
                    original.as_deref(),
                    TurnOutcome::Failed {
                        error: "The AI edited the script but never ran it successfully."
                            .to_string(),
                    },
                ),
                None => TurnOutcome::Answered,
            },
        }
    }

    /// Ask the same configured model to retain the information that makes a
    /// resumed project useful before old detail uses the available context.
    async fn condense(&self, conversation: &Conversation) -> Result<String, TurnOutcome> {
        let request = ModelRequest {
            purpose: RequestPurpose::Condense,
            instructions: "Condense this earlier CAD conversation for the next turn. Retain every decision already made and every request still open, including dimensions, constraints, and unresolved questions. Drop only chatter and completed detail. Return the compact account alone; do not call tools.".to_string(),
            items: history_items(conversation.messages(), self.model.accepts_images()),
            tools: Vec::new(),
        };
        let mut sink = |_delta: StreamDelta| {};
        match self.model.respond(request, self.cancel.clone(), &mut sink).await {
            Ok(response) if response.reached_output_limit => Err(TurnOutcome::Failed {
                error: "The conversation summary reached the provider's output limit before it was finished; the earlier conversation was kept."
                    .to_string(),
            }),
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
        attachments: &[ImageAttachment],
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
                let code = match args.code {
                    Some(code) => {
                        if let Err(error) = std::fs::write(&self.script_path, &code) {
                            return ToolRun::Abort(TurnOutcome::Failed {
                                error: format!("could not write the script: {error}"),
                            });
                        }
                        code
                    }
                    None => match std::fs::read_to_string(&self.script_path) {
                        Ok(code) => code,
                        Err(_) => {
                            return ToolRun::Output {
                                output: "There is no script to run yet: pass the complete \
                                         file in `code`."
                                    .to_string(),
                                failed: true,
                            };
                        }
                    },
                };
                match self.executor.execute(&self.script_path, &self.cancel) {
                    Ok(model) => ToolRun::Built {
                        code,
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
            EDIT_SCRIPT => {
                let args: EditScriptArgs = match serde_json::from_value(call.arguments.clone()) {
                    Ok(args) => args,
                    Err(error) => return ToolRun::bad_arguments(error),
                };
                emit(TurnEvent::Phase("editing the script".to_string()));
                let Ok(source) = std::fs::read_to_string(&self.script_path) else {
                    return ToolRun::Output {
                        output: "There is no script to edit yet: create it with `run_script` \
                                 and the complete file in `code`."
                            .to_string(),
                        failed: true,
                    };
                };
                match apply_edit(&source, &args.old_text, &args.new_text, args.replace_all) {
                    Ok(edit) => {
                        if let Err(error) = std::fs::write(&self.script_path, &edit.source) {
                            return ToolRun::Abort(TurnOutcome::Failed {
                                error: format!("could not write the script: {error}"),
                            });
                        }
                        ToolRun::Output {
                            output: edit.report,
                            failed: false,
                        }
                    }
                    Err(reason) => ToolRun::Output {
                        output: reason,
                        failed: true,
                    },
                }
            }
            READ_SCRIPT => {
                let args: ReadScriptArgs = match serde_json::from_value(call.arguments.clone()) {
                    Ok(args) => args,
                    Err(error) => return ToolRun::bad_arguments(error),
                };
                emit(TurnEvent::Phase("reading the script".to_string()));
                match std::fs::read_to_string(&self.script_path) {
                    Ok(source) => ToolRun::Output {
                        output: numbered_lines(&source, args.start_line, args.end_line),
                        failed: false,
                    },
                    Err(_) => ToolRun::Output {
                        output: "There is no script yet.".to_string(),
                        failed: true,
                    },
                }
            }
            RUN_PYTHON => {
                let args: RunPythonArgs = match serde_json::from_value(call.arguments.clone()) {
                    Ok(args) => args,
                    Err(error) => return ToolRun::bad_arguments(error),
                };
                emit(TurnEvent::Phase("running a Python snippet".to_string()));
                let script = (!args.standalone).then_some(self.script_path.as_path());
                if script.is_some_and(|path| !path.exists()) {
                    return ToolRun::Output {
                        output: "There is no script yet for the snippet to run after: set \
                                 `standalone`, or create the script with `run_script` first."
                            .to_string(),
                        failed: true,
                    };
                }
                match self.executor.run_snippet(script, &args.code, &self.cancel) {
                    Ok(outcome) => ToolRun::Output {
                        failed: outcome.error.is_some(),
                        output: describe_snippet(&outcome),
                    },
                    Err(error) if error.is_script_fault() => ToolRun::Output {
                        output: format!("The snippet was stopped:\n{error}"),
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
                    "looking at the render {}",
                    describe_render(&args)
                )));
                match self.render.render(args.view, args.part.as_deref()) {
                    Ok(image) => ToolRun::Rendered { image },
                    Err(reason) => ToolRun::Output {
                        output: format!("render unavailable: {reason}"),
                        failed: true,
                    },
                }
            }
            REFERENCE_IMAGES => {
                let args: ReferenceImagesArgs = match serde_json::from_value(call.arguments.clone())
                {
                    Ok(args) => args,
                    Err(error) => return ToolRun::bad_arguments(error),
                };
                match args.file {
                    None => {
                        emit(TurnEvent::Phase("listing the reference images".to_string()));
                        match self.references.list() {
                            Ok(entries) => ToolRun::Output {
                                output: describe_references(&entries),
                                failed: false,
                            },
                            Err(reason) => ToolRun::Output {
                                output: format!("reference library unavailable: {reason}"),
                                failed: true,
                            },
                        }
                    }
                    Some(file) => {
                        emit(TurnEvent::Phase(format!("looking at {file}")));
                        match self.references.read(&file) {
                            Ok(image) => ToolRun::ImageRead { file, image },
                            Err(reason) => ToolRun::Output {
                                output: format!("could not read {file}: {reason}"),
                                failed: true,
                            },
                        }
                    }
                }
            }
            KEEP_REFERENCE => {
                let args: KeepReferenceArgs = match serde_json::from_value(call.arguments.clone()) {
                    Ok(args) => args,
                    Err(error) => return ToolRun::bad_arguments(error),
                };
                emit(TurnEvent::Phase(
                    "updating the reference library".to_string(),
                ));
                let result = match &args.source {
                    KeepReferenceSource::Attachment { file } => {
                        match attachments
                            .iter()
                            .find(|attachment| &attachment.file == file)
                        {
                            Some(attachment) => match attachment.image() {
                                Some(image) => self
                                    .references
                                    .keep(
                                        &image,
                                        args.file.as_deref().unwrap_or(&attachment.name),
                                        &args.description,
                                    )
                                    .map(|kept| {
                                        format!("kept as {kept} and described in the index")
                                    }),
                                None => {
                                    Err(format!("the attachment {file} has no image data any more"))
                                }
                            },
                            None => Err(format!(
                                "no attachment {file} is on any message in this conversation; \
                                 each message's [Attached images: …] line gives its images' \
                                 file names in brackets"
                            )),
                        }
                    }
                    KeepReferenceSource::Library { file } => self
                        .references
                        .describe(file, &args.description)
                        .map(|()| format!("{file} is described in the index")),
                };
                match result {
                    Ok(output) => ToolRun::Output {
                        output,
                        failed: false,
                    },
                    Err(reason) => ToolRun::Output {
                        output: format!("could not update the reference library: {reason}"),
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
    /// A reference image read from the library, delivered the same way.
    ImageRead {
        file: String,
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

/// An `edit_script` applied: the new source, and the report the model
/// reads — each edited region with line numbers.
#[derive(Debug)]
struct AppliedEdit {
    source: String,
    report: String,
}

/// Lines of context shown either side of an edited region.
const EDIT_CONTEXT_LINES: usize = 2;
/// How many edited regions a `replace_all` report shows in full.
const EDIT_REGIONS_SHOWN: usize = 5;

/// Replace `old_text` in `source` with `new_text`. Exactly one occurrence
/// is required unless `replace_all`; the refusals say what to do instead,
/// since the model reads them.
fn apply_edit(
    source: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> Result<AppliedEdit, String> {
    if old_text.is_empty() {
        return Err("`old_text` is empty: give the exact text to replace.".to_string());
    }
    let count = source.matches(old_text).count();
    if count == 0 {
        return Err(
            "`old_text` was not found in the script. Copy it exactly from the file as \
                    it now stands, whitespace included: `read_script` shows it with line \
                    numbers, and the <current_script> block shows it only as the turn began."
                .to_string(),
        );
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "`old_text` occurs {count} times; include more surrounding lines to make it \
             unique, or set `replace_all` to change every occurrence."
        ));
    }
    let mut edited = String::with_capacity(source.len());
    let mut starts = Vec::with_capacity(count);
    let mut rest = source;
    while let Some(at) = rest.find(old_text) {
        edited.push_str(&rest[..at]);
        starts.push(edited.len());
        edited.push_str(new_text);
        rest = &rest[at + old_text.len()..];
    }
    edited.push_str(rest);

    let total = edited.lines().count();
    let mut report = format!(
        "Replaced {count} occurrence{}. The script is now {total} line{}.",
        if count == 1 { "" } else { "s" },
        if total == 1 { "" } else { "s" },
    );
    let new_lines = new_text.matches('\n').count();
    for start in starts.iter().take(EDIT_REGIONS_SHOWN) {
        let first = edited[..*start].matches('\n').count() + 1;
        let last = first + new_lines;
        report.push('\n');
        report.push_str(&numbered_lines(
            &edited,
            Some(first.saturating_sub(EDIT_CONTEXT_LINES).max(1) as u32),
            Some((last + EDIT_CONTEXT_LINES) as u32),
        ));
    }
    if count > EDIT_REGIONS_SHOWN {
        report.push_str(&format!("\n… and {} more.", count - EDIT_REGIONS_SHOWN));
    }
    Ok(AppliedEdit {
        source: edited,
        report,
    })
}

/// `source` between two 1-based line numbers, inclusive, each line
/// prefixed with its number as a traceback names it. Either bound absent
/// means that end of the file; a range past the end says how long the
/// file is instead.
fn numbered_lines(source: &str, start_line: Option<u32>, end_line: Option<u32>) -> String {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() {
        return "The script is empty.".to_string();
    }
    let first = start_line.unwrap_or(1).max(1) as usize;
    let last = (end_line.map_or(lines.len(), |end| end as usize)).min(lines.len());
    if first > last {
        return format!(
            "The script has {} line{}; nothing to show from line {first}.",
            lines.len(),
            if lines.len() == 1 { "" } else { "s" }
        );
    }
    let width = last.to_string().len();
    lines[first - 1..last]
        .iter()
        .enumerate()
        .map(|(offset, line)| format!("{:>width$} | {line}", first + offset))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What the model reads after a snippet: what it printed, the value of
/// its final expression, and the traceback if it raised, each only when
/// present.
fn describe_snippet(outcome: &SnippetOutcome) -> String {
    let mut parts = Vec::new();
    if !outcome.printed.trim().is_empty() {
        parts.push(format!("Printed:\n{}", outcome.printed.trim_end()));
    }
    if let Some(value) = &outcome.value {
        parts.push(format!("Value: {value}"));
    }
    if let Some(error) = &outcome.error {
        parts.push(format!("Raised:\n{error}"));
    }
    if parts.is_empty() {
        "Ran with no output: nothing printed and no final expression.".to_string()
    } else {
        parts.join("\n")
    }
}

/// The turn's opening message: chat text and every comment with its
/// anchors, naming the images attached.
fn render_input(input: &TurnInput) -> String {
    let mut parts: Vec<String> = input.comments.iter().map(render_comment).collect();
    if let Some(chat) = &input.chat {
        parts.push(chat.clone());
    }
    with_attachment_names(&parts.join("\n\n"), &input.images)
}

/// Name the images that go with a message, each with its stored file
/// name, so the model knows which is which when several are attached and
/// can refer to one unambiguously — to `keep_reference` — even when two
/// share the name the user knows them by.
fn with_attachment_names(text: &str, attachments: &[ImageAttachment]) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let names = attachments
        .iter()
        .map(|attachment| format!("\"{}\" ({})", attachment.name, attachment.file))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{text}\n\n[Attached images: {names}]")
}

/// The reference library as the listing tool reports it.
fn describe_references(entries: &[ReferenceListing]) -> String {
    if entries.is_empty() {
        return "The reference library is empty.".to_string();
    }
    let mut text = String::from(
        "Images in the reference library (call again with a file name to look at one):\n",
    );
    for entry in entries {
        text.push_str(&format!(
            "- {}: {}\n",
            entry.file,
            entry
                .description
                .as_deref()
                .unwrap_or("(no description yet; look at it and describe it with keep_reference)")
        ));
    }
    text
}

/// The last script the model ran successfully in this conversation, as
/// the history records it. `None` when the history holds no such run: a
/// new conversation, or one condensed past its last run.
fn last_successful_run(conversation: &Conversation) -> Option<&str> {
    conversation
        .messages()
        .iter()
        .rev()
        .find_map(|message| match &message.kind {
            MessageKind::ToolCalls(activities) => activities.iter().rev().find_map(|activity| {
                (activity.tool == RUN_SCRIPT && !activity.failed && activity.output.is_some())
                    .then(|| {
                        // The record of what ran; conversations saved
                        // before it was kept carry the whole file in the
                        // call's arguments instead.
                        activity.executed_source.as_deref().or_else(|| {
                            activity
                                .arguments
                                .get("code")
                                .and_then(serde_json::Value::as_str)
                        })
                    })
                    .flatten()
            }),
            _ => None,
        })
}

/// The script the model must start from, and how it relates to the last
/// version the model ran. When the file differs from that run, the
/// parameter values that changed are named, so a value the user set in
/// the panel is kept rather than overwritten from memory.
fn current_script_block(on_disk: Option<&str>, last_run: Option<&str>) -> String {
    let Some(script) = on_disk else {
        return "There is no script yet: the part is empty, and `run_script` with the complete \
                file in `code` creates it."
            .to_string();
    };
    let mut out = String::from("The current script, as it stands on disk. ");
    match last_run {
        Some(last_run) if last_run == script => {
            out.push_str("It is unchanged since your last run in this conversation.");
        }
        Some(last_run) => {
            out.push_str(
                "It differs from the last script you ran: it was changed outside this \
                 conversation (a parameter edited in the panel, a design step undone or \
                 redone, or a turn whose run was not kept). Start from this text, not \
                 from your last run.",
            );
            let changes = changed_parameter_values(last_run, script);
            if !changes.is_empty() {
                out.push_str("\nParameter values that changed: ");
                out.push_str(&changes.join(", "));
            }
        }
        None => out.push_str(
            "This conversation holds no run of it, so this text is the only authoritative \
             version; start from it, not from memory.",
        ),
    }
    out.push_str(&format!("\n<current_script>\n{script}\n</current_script>"));
    out
}

/// Each parameter whose literal value differs between two versions of the
/// script, as `name old → new`, in the current script's order.
fn changed_parameter_values(before: &str, after: &str) -> Vec<String> {
    let before = script_parameters::extract(before);
    script_parameters::extract(after)
        .iter()
        .filter_map(|parameter| {
            let new = parameter.value()?;
            let old = before
                .iter()
                .find(|earlier| earlier.name == parameter.name)?
                .value()?;
            (old != new).then(|| format!("{} {old} → {new}", parameter.name))
        })
        .collect()
}

/// What became of the parameters `before` had locked, in `after`: one
/// entry per locked parameter that was removed, unlocked, changed while
/// still locked, or newly changed after its binding by an augmented
/// assignment (`wall += 1`), in the order `before` lists them. A lock
/// added in `after` is not a change. Empty when nothing locked was
/// touched, or when there was no script before.
fn locked_parameter_changes(before: Option<&str>, after: &str) -> Vec<String> {
    let Some(before) = before else {
        return Vec::new();
    };
    let was_augmented = script_parameters::augmentations(before);
    let augmented = script_parameters::augmentations(after);
    let after = script_parameters::extract(after);
    script_parameters::extract(before)
        .into_iter()
        .filter_map(|was| {
            let lock = was.lock.as_ref()?;
            let name = &was.name;
            let reason = lock
                .reason
                .as_deref()
                .map(|reason| format!(", locked because: {reason}"))
                .unwrap_or_default();
            let now = after.iter().find(|now| now.name == *name);
            Some(match now {
                None => format!("`{name}` was removed (it was locked{reason})"),
                Some(now) if now.lock.is_none() && now.stated() != was.stated() => format!(
                    "`{name}` was unlocked and changed {} → {}{reason}",
                    was.stated(),
                    now.stated()
                ),
                Some(now) if now.lock.is_none() => {
                    format!("`{name}` was unlocked (its `# locked` marker was removed{reason})")
                }
                Some(now) if now.stated() != was.stated() => format!(
                    "`{name}` changed {} → {} while locked{reason}",
                    was.stated(),
                    now.stated()
                ),
                Some(_) => {
                    // The binding stands, but a statement the script did
                    // not have before now changes the name in place.
                    let added = augmented
                        .iter()
                        .find(|now| now.name == *name && !was_augmented.contains(now))?;
                    format!(
                        "`{name}` is changed after its binding by `{}` (line {}) while locked{reason}",
                        added.text, added.line
                    )
                }
            })
        })
        .collect()
}

/// Every replayed tool call needs a result, including calls interrupted by Cancel.
fn settle_unanswered_calls(items: &mut Vec<ModelItem>) {
    let pending: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            ModelItem::ToolCall(call) => Some(call.id.clone()),
            ModelItem::ProviderOutput(item) if item["type"] == "function_call" => {
                item["call_id"].as_str().map(str::to_string)
            }
            _ => None,
        })
        .filter(|id| {
            !items.iter().any(|item| {
                matches!(item,
                    ModelItem::ToolResult { call_id, .. } if call_id == id
                )
            })
        })
        .collect();
    for call_id in pending {
        items.push(ModelItem::ToolResult {
            call_id,
            output: "The turn ended before this call returned a result. Check the current script before making further edits.".into(),
        });
    }
}

/// The conversation so far as the model sees it.
/// The saved conversation as the model is shown it. A user message's
/// attached images ride with it again, so a picture the user gave three
/// turns ago is still in front of the model; a model that cannot read
/// images is shown the text alone.
fn history_items(messages: &[Message], accepts_images: bool) -> Vec<ModelItem> {
    let mut items = Vec::new();
    for message in messages {
        let images = if accepts_images {
            message.images()
        } else {
            Vec::new()
        };
        match &message.kind {
            MessageKind::UserChat => items.push(ModelItem::User {
                text: with_attachment_names(&message.text, &message.attachments),
                images,
            }),
            MessageKind::SpatialComment { anchors, .. } => items.push(ModelItem::User {
                text: with_attachment_names(
                    &render_comment(&GroundedComment {
                        text: message.text.clone(),
                        anchors: anchors.clone(),
                    }),
                    &message.attachments,
                ),
                images,
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
            MessageKind::DesignChange => items.push(ModelItem::User {
                text: format!("Note from CADmark: {}", message.text),
                images: Vec::new(),
            }),
            MessageKind::Notice { .. } | MessageKind::Thinking { .. } => {}
        }
    }
    items
}

/// Everything one request carries besides the conversation history: the
/// instructions (with any active skill), the tool definitions, the current
/// script, the example library the request selects, and the request text
/// itself. Built once per turn for the model, and again from the draft for
/// the occupancy figure the chat shows, so the two never disagree about
/// what a request weighs.
pub struct RequestAssembly {
    pub instructions: String,
    pub tools: Vec<ToolSpec>,
    /// The items that follow the history: the script block, the examples,
    /// and the request, in that order.
    pub items: Vec<ModelItem>,
    conversation_tokens: usize,
    image_count: usize,
}

impl RequestAssembly {
    pub fn new(
        conversation: &Conversation,
        script_on_disk: Option<&str>,
        input: &TurnInput,
        accepts_images: bool,
    ) -> Self {
        let last_run = last_successful_run(conversation);
        let selected_skills = skills::for_turn(
            input
                .chat
                .iter()
                .map(String::as_str)
                .chain(input.comments.iter().map(|comment| comment.text.as_str())),
        );
        let request = render_input(input);
        // The script on disk is the design. It is attached to every request
        // because the history cannot be trusted to carry it: a fresh or
        // condensed conversation holds no run of it, and a parameter edit,
        // an undo, or a cancelled turn leaves the file different from the
        // last run the history does hold. The curated example library for
        // the operations the request names comes next, so the request
        // itself stays last.
        let mut items = vec![
            ModelItem::User {
                text: current_script_block(script_on_disk, last_run),
                images: Vec::new(),
            },
            ModelItem::User {
                text: examples::context_block(&request),
                images: Vec::new(),
            },
            ModelItem::User {
                text: request,
                // A model that cannot read images is told their names
                // and nothing else; the bytes would be refused.
                images: if accepts_images {
                    input
                        .images
                        .iter()
                        .filter_map(ImageAttachment::image)
                        .collect()
                } else {
                    Vec::new()
                },
            },
        ];
        let instructions = instructions_for(accepts_images);
        for skill in selected_skills {
            items.insert(items.len() - 1, ModelItem::Developer {
                text: format!(
                    "# Active skill: {}\n\nApply this skill only to the following user request and its tool loop.\n{}",
                    skill.name, skill.instructions,
                ),
            });
        }
        Self {
            instructions,
            tools: tools_for(accepts_images),
            items,
            conversation_tokens: conversation.estimated_tokens(),
            image_count: if accepts_images {
                input.images.len()
            } else {
                0
            },
        }
    }

    /// What the request weighs against a context window: the history, the
    /// images, and everything else assembled here. Text is estimated
    /// provider-neutrally, and the tool definitions as their JSON.
    pub fn usage(&self, window_tokens: usize) -> ContextUsage {
        let items: usize = self
            .items
            .iter()
            .map(|item| match item {
                ModelItem::User { text, .. } | ModelItem::Assistant { text } => {
                    estimate_tokens(text)
                }
                ModelItem::ToolCall(call) => {
                    estimate_tokens(&call.name) + estimate_tokens(&call.arguments.to_string())
                }
                ModelItem::ToolResult { output, .. } => estimate_tokens(output),
                ModelItem::ProviderOutput(_) | ModelItem::Developer { .. } => {
                    item.estimated_tokens()
                }
            })
            .sum();
        let tools: usize = self
            .tools
            .iter()
            .map(|tool| {
                estimate_tokens(&tool.name)
                    + estimate_tokens(&tool.description)
                    + estimate_tokens(&tool.parameters.to_string())
            })
            .sum();
        ContextUsage {
            conversation_tokens: self.conversation_tokens,
            image_tokens: self.image_count * IMAGE_TOKENS,
            request_tokens: estimate_tokens(&self.instructions) + tools + items,
            window_tokens,
        }
    }
}

/// What the model reads after a successful execution: the whole model's
/// measurements, each part's own when the script completed several, and
/// whether each is a closed valid solid.
fn describe_model(model: &ExecutedModel) -> String {
    let mut text = match &model.form {
        ModelForm::Solid(solid) => {
            let mut text = format!(
                "Executed successfully. Model: {}.",
                solid.summary.describe()
            );
            if solid.parts.len() > 1 {
                text.push_str(&format!(
                    " Parts: {}.",
                    describe_parts(&solid.part_measurements())
                ));
            }
            text.push(' ');
            text.push_str(&describe_validity(&solid.validity));
            text
        }
        // A sketch has no volume, no faces and no validity to report: it
        // is drawn on screen, and extruding it is the next step.
        ModelForm::Sketch(sketch) => format!(
            "Executed successfully. {}. It is drawn in the viewport and can be \
             exported as an SVG or DXF drawing or as STEP; it has no volume to \
             measure until it becomes a solid.",
            sketch.profile.describe()
        ),
    };
    let untraced = model.ledger.untraced_count();
    if untraced > 0 {
        text.push_str(&format!(
            " {untraced} elements came from operations CADmark cannot trace."
        ));
    }
    if !model.printed.trim().is_empty() {
        text.push_str(&format!("\nPrinted:\n{}", model.printed.trim_end()));
    }
    text
}

fn describe_tool(name: &str) -> &str {
    match name {
        RUN_SCRIPT => "run the script",
        EDIT_SCRIPT => "edit the script",
        READ_SCRIPT => "read the script",
        RUN_PYTHON => "run a Python snippet",
        LOOKUP_DOCS => "look up the docs",
        RENDER_VIEW => "look at the render",
        REFERENCE_IMAGES => "look at the reference images",
        KEEP_REFERENCE => "update the reference library",
        other => other,
    }
}

/// The instructions one turn is run with: the system prompt, plus what
/// the model is owed about any tool it is not being offered.
fn instructions_for(accepts_images: bool) -> String {
    match unavailable_tools_note(accepts_images) {
        Some(note) => format!("{SYSTEM_PROMPT}\n\n{note}"),
        None => SYSTEM_PROMPT.to_string(),
    }
}

/// What a render call asked for, for the caption; a call whose arguments
/// did not parse never reaches here.
fn render_args_of(arguments: &serde_json::Value) -> RenderViewArgs {
    serde_json::from_value::<RenderViewArgs>(arguments.clone()).unwrap_or(RenderViewArgs {
        view: RenderView::Current,
        part: None,
    })
}

/// A render request in words, for the phase line and the image's caption:
/// "from the front", or "of `lid` alone from the front".
fn describe_render(args: &RenderViewArgs) -> String {
    match &args.part {
        Some(part) => format!("of `{part}` alone from the {}", describe_view(args.view)),
        None => format!("from the {}", describe_view(args.view)),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use crate::render_source::{RenderGpu, SceneHandle, ScenePart, ViewportRender};
    use cadmark_bridge::backend::{DeltaSink, ModelResponse};
    use cadmark_core::geometry::{GeometryDescriptors, ModelSummary, SolidValidity};
    use cadmark_core::ledger::ProvenanceLedger;
    use cadmark_core::limits::{ExecutionLimits, LimitHit};
    use cadmark_core::mesh::TessellatedMesh;
    use cadmark_core::message::{Message, ToolActivity};
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
                        ModelItem::ToolCall(_)
                        | ModelItem::ToolResult { .. }
                        | ModelItem::ProviderOutput(_)
                        | ModelItem::Developer { .. } => "",
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
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
            text: reply.to_string(),
            tool_calls: Vec::new(),
        })
    }

    fn run_script(id: &str, code: &str, summary: &str) -> Result<ModelResponse, BackendError> {
        Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: RUN_SCRIPT.to_string(),
                arguments: serde_json::json!({"code": code, "summary": summary}),
            }],
        })
    }

    fn edit(id: &str, old_text: &str, new_text: &str) -> Result<ModelResponse, BackendError> {
        Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: EDIT_SCRIPT.to_string(),
                arguments: serde_json::json!({"old_text": old_text, "new_text": new_text}),
            }],
        })
    }

    /// A `run_script` without `code`: run the file as the edits left it.
    fn rerun(id: &str, summary: &str) -> Result<ModelResponse, BackendError> {
        Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: RUN_SCRIPT.to_string(),
                arguments: serde_json::json!({"summary": summary}),
            }],
        })
    }

    fn python(id: &str, code: &str, standalone: bool) -> Result<ModelResponse, BackendError> {
        Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: RUN_PYTHON.to_string(),
                arguments: serde_json::json!({"code": code, "standalone": standalone}),
            }],
        })
    }

    fn lookup(id: &str, query: &str) -> Result<ModelResponse, BackendError> {
        Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
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
    /// One snippet the fake executor was asked to run: the script it was
    /// to run after, and the code.
    type SnippetRun = (Option<PathBuf>, String);

    #[derive(Clone)]
    struct FakeExecutor {
        outcomes: Arc<Mutex<VecDeque<Result<(), WorkerError>>>>,
        executed: Arc<Mutex<Vec<String>>>,
        /// Scripted snippet outcomes, and each snippet run.
        snippets: Arc<Mutex<VecDeque<SnippetOutcome>>>,
        snippets_run: Arc<Mutex<Vec<SnippetRun>>>,
        block_restoration: bool,
    }

    impl FakeExecutor {
        fn new(outcomes: impl IntoIterator<Item = Result<(), WorkerError>>) -> Self {
            Self {
                outcomes: Arc::new(Mutex::new(outcomes.into_iter().collect())),
                executed: Arc::new(Mutex::new(Vec::new())),
                snippets: Arc::new(Mutex::new(VecDeque::new())),
                snippets_run: Arc::new(Mutex::new(Vec::new())),
                block_restoration: false,
            }
        }

        fn with_snippets(self, outcomes: impl IntoIterator<Item = SnippetOutcome>) -> Self {
            *self.snippets.lock().unwrap() = outcomes.into_iter().collect();
            self
        }

        fn snippets_run(&self) -> Vec<SnippetRun> {
            self.snippets_run.lock().unwrap().clone()
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

    fn sample_sketch() -> ExecutedModel {
        ExecutedModel {
            mesh: TessellatedMesh::default(),
            ledger: ProvenanceLedger::new(),
            sketch_lineage: Default::default(),
            descriptors: GeometryDescriptors::default(),
            printed: String::new(),
            form: ModelForm::Sketch(cadmark_kernel::protocol::SketchResult {
                profile: cadmark_core::sketch::SketchProfile {
                    plane: cadmark_core::sketch::SketchPlane {
                        origin: [0.0; 3],
                        normal: [0.0, 0.0, 1.0],
                        x_axis: [1.0, 0.0, 0.0],
                    },
                    curves: vec![cadmark_core::sketch::SketchCurve {
                        curve_id: 0,
                        points: vec![[0.0; 3], [1.0, 0.0, 0.0]],
                        curve_type: "line".to_string(),
                        length: 1.0,
                        radius: None,
                    }],
                    corners: vec![cadmark_core::sketch::SketchCorner {
                        corner_id: 0,
                        position: [0.0; 3],
                    }],
                    regions: Vec::new(),
                },
                file: ModelFile(PathBuf::from("/scratch/model-2.brep")),
            }),
        }
    }

    #[test]
    fn a_multi_part_result_tells_the_model_each_part_by_name() {
        let mut model = sample_model();
        let ModelForm::Solid(solid) = &mut model.form else {
            unreachable!("sample model is a solid");
        };
        let part = |id: u32, name: &str, volume: f64| cadmark_kernel::protocol::ExecutedPart {
            id,
            name: name.to_string(),
            mesh: TessellatedMesh::default(),
            ledger: ProvenanceLedger::new(),
            sketch_lineage: Default::default(),
            descriptors: GeometryDescriptors::default(),
            summary: ModelSummary {
                volume,
                ..solid.summary.clone()
            },
            validity: solid.validity.clone(),
            file: solid.file.clone(),
        };
        solid.parts = vec![part(0, "bracket", 1000.0), part(1, "lid", 200.0)];
        solid.validity = vec![solid.validity[0], solid.validity[0]];

        let text = describe_model(&model);

        assert!(
            text.contains(
                "Parts: bracket: 6 faces, volume 1000 mm³, 10 × 10 × 10 mm; \
                 lid: 6 faces, volume 200 mm³, 10 × 10 × 10 mm."
            ),
            "{text}"
        );
        assert!(text.contains("Part 2 is closed and valid."), "{text}");

        // One part is the model; naming it again would only repeat.
        let text = describe_model(&sample_model());
        assert!(!text.contains("Parts:"), "{text}");
    }

    #[test]
    fn a_sketch_only_result_is_reported_as_drawn_rather_than_as_a_solid() {
        let text = describe_model(&sample_sketch());

        assert!(text.starts_with("Executed successfully."), "{text}");
        assert!(text.contains("not yet a solid"), "{text}");
        assert!(text.contains("1 curve"), "{text}");
        assert!(text.contains("drawn in the viewport"), "{text}");
        assert!(text.contains("exported as an SVG or DXF drawing"), "{text}");
        assert!(text.contains("no volume to measure"), "{text}");
        // The solid vocabulary must not leak into a sketch's report.
        assert!(!text.contains("Volume"), "{text}");
        assert!(!text.contains("watertight"), "{text}");
    }

    fn sample_model() -> ExecutedModel {
        ExecutedModel {
            mesh: TessellatedMesh::default(),
            ledger: ProvenanceLedger::new(),
            sketch_lineage: Default::default(),
            descriptors: GeometryDescriptors::default(),
            printed: String::new(),
            form: ModelForm::Solid(cadmark_kernel::protocol::SolidResult {
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

        fn run_snippet(
            &mut self,
            script_path: Option<&Path>,
            code: &str,
            _cancel: &CancelFlag,
        ) -> Result<SnippetOutcome, WorkerError> {
            self.snippets_run
                .lock()
                .unwrap()
                .push((script_path.map(Path::to_path_buf), code.to_string()));
            Ok(self
                .snippets
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default())
        }
    }

    struct FakeRender;

    impl RenderSource for FakeRender {
        fn render(&mut self, _view: RenderView, _part: Option<&str>) -> Result<ImageData, String> {
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
            // The kernel reports every completed solid as a part, at
            // least one, and the parts are what a render draws.
            let mut model = sample_model();
            model.mesh = cube_mesh();
            if let ModelForm::Solid(solid) = &mut model.form {
                solid.parts = vec![cadmark_kernel::protocol::ExecutedPart {
                    id: 0,
                    name: "cube".into(),
                    mesh: cube_mesh(),
                    ledger: ProvenanceLedger::new(),
                    sketch_lineage: Default::default(),
                    descriptors: GeometryDescriptors::default(),
                    summary: solid.summary.clone(),
                    validity: solid.validity.clone(),
                    file: solid.file.clone(),
                }];
            }
            Ok(model)
        }

        fn run_snippet(
            &mut self,
            _script_path: Option<&Path>,
            _code: &str,
            _cancel: &CancelFlag,
        ) -> Result<SnippetOutcome, WorkerError> {
            Ok(SnippetOutcome::default())
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
                references: &NoReferences,
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
                references: &NoReferences,
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
    async fn printing_skill_reaches_every_model_request_with_design_context() {
        for command in ["/3d-printing", "$3d-printing"] {
            let model = ScriptedModel::new([
                lookup("docs", "threaded bolt helix sweep"),
                text("Use a vertical axis and test the fit."),
                text("Ordinary follow-up."),
            ]);
            let original = "from build123d import *\npart = Box(20, 10, 2)\n";
            let mut harness = Harness::with_script(Some(original), FakeExecutor::new([]));
            let input = TurnInput {
                chat: Some(format!("{command} Review this bracket")),
                comments: vec![GroundedComment {
                    text: "$3d-printing Check the holes too".into(),
                    anchors: Vec::new(),
                }],
                ..Default::default()
            };
            assert_eq!(
                harness.run(&model, input, CancelFlag::new()).await,
                TurnOutcome::Answered
            );
            assert_eq!(harness.on_disk().as_deref(), Some(original));
            {
                let requests = model.requests.lock().unwrap();
                assert_eq!(requests.len(), 2);
                for request in requests.iter() {
                    let active: Vec<_> = request
                        .items
                        .iter()
                        .filter_map(|item| match item {
                            ModelItem::Developer { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    assert_eq!(active.len(), 1);
                    assert!(active[0].contains(skills::BUILT_IN[0].instructions));
                    assert!(active[0].contains("only to the following user request"));
                    assert!(!request.instructions.contains("# Active skill:"));
                    assert!(request.items.iter().any(|item| matches!(item,
                        ModelItem::User { text, .. } if text.contains(original))));
                    assert!(request.items.iter().any(|item| matches!(item,
                        ModelItem::User { text, .. } if text.contains("Review this bracket") && text.contains("Check the holes too"))));
                }
            }
            let mut history = Conversation::new();
            history.push(Message::user_chat(format!("{command} Review this bracket")));
            harness
                .run_with_conversation(
                    &model,
                    &history,
                    chat("Explain the dimensions"),
                    CancelFlag::new(),
                )
                .await;
            let requests = model.requests.lock().unwrap();
            assert_eq!(requests[0].instructions, requests[2].instructions);
            assert!(
                !requests[2]
                    .items
                    .iter()
                    .any(|item| matches!(item, ModelItem::Developer { .. }))
            );
        }
    }

    #[tokio::test]
    async fn a_turn_ends_by_reporting_the_item_sequence_it_sent_with_its_reply() {
        let model = ScriptedModel::new([text("All done.")]);
        let mut harness =
            Harness::with_script(Some("part = Box(10, 10, 2)"), FakeExecutor::new([]));
        let input = TurnInput {
            chat: Some("Is it done?".into()),
            ..Default::default()
        };
        harness
            .run_with_conversation(&model, &Conversation::new(), input, CancelFlag::new())
            .await;

        let Some(TurnEvent::ModelContext { items, .. }) = harness.events.last() else {
            panic!(
                "the last event names the model context: {:?}",
                harness.events.last()
            );
        };
        let sent = &model.requests.lock().unwrap()[0].items;
        assert_eq!(&items[..sent.len()], sent.as_slice());
        assert_eq!(
            items[sent.len()..],
            [ModelItem::Assistant {
                text: "All done.".into()
            }]
        );
    }

    #[tokio::test]
    async fn the_next_turn_begins_with_the_recorded_session_and_renders_only_what_came_after() {
        let mut history = Conversation::new();
        history.push(Message::user_chat(
            "RENDERED-BY-THE-CHAT would be the wrong source",
        ));
        history.push(Message::ai_response("Done."));
        let recorded = vec![
            ModelItem::User {
                text: "EXACT-ITEM the model last saw".into(),
                images: Vec::new(),
            },
            ModelItem::Assistant {
                text: "Done.".into(),
            },
        ];
        history.record_session(recorded.clone());
        history.push(Message::design_change("the user undid a step"));

        let model = ScriptedModel::new([text("Noted.")]);
        let mut harness =
            Harness::with_script(Some("part = Box(10, 10, 2)"), FakeExecutor::new([]));
        let input = TurnInput {
            chat: Some("Carry on.".into()),
            ..Default::default()
        };
        harness
            .run_with_conversation(&model, &history, input, CancelFlag::new())
            .await;

        let requests = model.requests.lock().unwrap();
        let items = &requests[0].items;
        assert_eq!(
            &items[..2],
            recorded.as_slice(),
            "the session is sent as recorded"
        );
        let texts: Vec<&str> = items
            .iter()
            .map(|item| match item {
                ModelItem::User { text, .. } | ModelItem::Assistant { text } => text.as_str(),
                _ => "",
            })
            .collect();
        assert!(
            texts[2].contains("the user undid a step"),
            "the message after the session is rendered next: {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("RENDERED-BY-THE-CHAT")),
            "messages the session accounts for are not rendered again: {texts:?}"
        );
        assert!(texts.last().unwrap().contains("Carry on."));
    }

    #[tokio::test]
    async fn ordered_reasoning_and_calls_survive_tools_save_and_a_follow_up_with_a_skill() {
        let first_call = ToolCall {
            id: "first".into(),
            name: LOOKUP_DOCS.into(),
            arguments: serde_json::json!({"query":"loft"}),
        };
        let second_call = ToolCall {
            id: "second".into(),
            name: LOOKUP_DOCS.into(),
            arguments: serde_json::json!({"query":"sweep"}),
        };
        let output = vec![
            ModelItem::ProviderOutput(serde_json::json!({"type":"reasoning","id":"rs_1",
                "encrypted_content":"signed-one","summary":[]})),
            ModelItem::ProviderOutput(serde_json::json!({"type":"message","role":"assistant",
                "content":[{"type":"output_text","text":"Looking up both."}]})),
            ModelItem::ProviderOutput(serde_json::json!({"type":"function_call","call_id":"first",
                "name":LOOKUP_DOCS,"arguments":"{\"query\":\"loft\"}"})),
            ModelItem::ProviderOutput(serde_json::json!({"type":"reasoning","id":"rs_2",
                "encrypted_content":"signed-two","summary":[]})),
            ModelItem::ProviderOutput(
                serde_json::json!({"type":"function_call","call_id":"second",
                "name":LOOKUP_DOCS,"arguments":"{\"query\":\"sweep\"}"}),
            ),
        ];
        let model = ScriptedModel::new([
            Ok(ModelResponse {
                output_items: output.clone(),
                text: "Looking up both.".into(),
                tool_calls: vec![first_call, second_call],
                ..Default::default()
            }),
            text("Ready."),
            text("Follow-up."),
        ]);
        let mut harness =
            Harness::with_script(Some("part = Box(20, 10, 2)"), FakeExecutor::new([]));
        assert_eq!(
            harness
                .run(&model, chat("Check both methods"), CancelFlag::new())
                .await,
            TurnOutcome::Answered
        );
        let initial_count = model.requests.lock().unwrap()[0].items.len();
        {
            let requests = model.requests.lock().unwrap();
            let second = &requests[1].items;
            assert_eq!(&second[..initial_count], requests[0].items.as_slice());
            assert_eq!(
                &second[initial_count..initial_count + output.len()],
                output.as_slice()
            );
            assert!(
                matches!(&second[initial_count+5], ModelItem::ToolResult {call_id,..} if call_id=="first")
            );
            assert!(
                matches!(&second[initial_count+6], ModelItem::ToolResult {call_id,..} if call_id=="second")
            );
        }
        let Some(TurnEvent::ModelContext { items, .. }) = harness.events.last() else {
            panic!("no session")
        };
        let saved_items = items.clone();
        let mut history = Conversation::new();
        history.push(Message::user_chat("Check both methods"));
        history.push(Message::ai_response("Ready."));
        history.record_session(saved_items.clone());
        let file = harness.script.with_file_name("conversation.json");
        std::fs::write(&file, serde_json::to_vec(&history).unwrap()).unwrap();
        let history: Conversation = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        harness
            .run_with_conversation(
                &model,
                &history,
                chat("/3d-printing Check the clearances"),
                CancelFlag::new(),
            )
            .await;
        let requests = model.requests.lock().unwrap();
        assert_eq!(
            &requests[2].items[..saved_items.len()],
            saved_items.as_slice()
        );
        assert_eq!(requests[0].instructions, requests[2].instructions);
        assert_eq!(requests[0].tools, requests[2].tools);
        assert!(
            requests[2].items[saved_items.len()..]
                .iter()
                .any(|item| matches!(item,
            ModelItem::Developer {text} if text.contains(skills::BUILT_IN[0].instructions)))
        );
    }

    #[tokio::test]
    async fn an_output_limit_continues_from_complete_opaque_reasoning_without_visible_text() {
        let reasoning = ModelItem::ProviderOutput(serde_json::json!({"type":"reasoning",
            "id":"rs_1","status":"completed","summary":[],"encrypted_content":"signed"}));
        let model = ScriptedModel::new([
            Ok(ModelResponse {
                output_items: vec![reasoning.clone()],
                reached_output_limit: true,
                ..Default::default()
            }),
            text("Continued."),
        ]);
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([]));
        assert_eq!(
            harness
                .run(&model, chat("Plan the splits"), CancelFlag::new())
                .await,
            TurnOutcome::Answered
        );
        let requests = model.requests.lock().unwrap();
        let tail = &requests[1].items[requests[0].items.len()..];
        assert_eq!(tail[0], reasoning);
        assert!(matches!(&tail[1], ModelItem::User {text,..} if text==CONTINUE_AFTER_OUTPUT_LIMIT));
    }

    #[tokio::test]
    async fn cancelling_before_tools_finish_leaves_paired_results_for_replay() {
        let cancel = CancelFlag::new();
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([]));
        let response = lookup("pending", "loft").unwrap();
        let model = ScriptedModel::new([Ok(response)]);
        let mut render = NoRender;
        // Cancellation immediately after the completed response prevents the tool
        // running but its call still belongs to the replayed provider output.
        let model = CancelAfterResponse {
            model,
            cancel: cancel.clone(),
        };
        let mut runner = TurnRunner {
            model: &model,
            executor: &mut harness.executor,
            docs: &FakeDocs,
            render: &mut render,
            references: &NoReferences,
            script_path: harness.script.clone(),
            cancel,
        };
        let mut items = vec![];
        let outcome = runner
            .run(&Conversation::new(), &chat("Check"), |event| {
                if let TurnEvent::ModelContext { items: context, .. } = event {
                    items = context;
                }
            })
            .await;
        assert_eq!(outcome, TurnOutcome::Cancelled);
        assert!(
            items
                .iter()
                .any(|item| matches!(item, ModelItem::ToolCall(call) if call.id=="pending"))
        );
        assert!(
            items
                .iter()
                .any(|item| matches!(item, ModelItem::ToolResult {call_id,output}
            if call_id=="pending" && output.contains("before this call returned")))
        );
    }

    struct CancelAfterResponse {
        model: ScriptedModel,
        cancel: CancelFlag,
    }
    impl TurnModel for CancelAfterResponse {
        fn model_name(&self) -> &str {
            "cancel-after-response"
        }
        fn accepts_images(&self) -> bool {
            false
        }
        fn respond<'a>(
            &'a self,
            request: ModelRequest,
            cancel: CancelFlag,
            sink: DeltaSink<'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<ModelResponse, BackendError>> + Send + 'a>,
        > {
            Box::pin(async move {
                let response = self.model.respond(request, cancel, sink).await;
                self.cancel.cancel();
                response
            })
        }
    }

    #[tokio::test]
    async fn the_usage_a_provider_reports_is_a_turn_event() {
        let usage = ProviderUsage {
            input_tokens: 900,
            cached_input_tokens: Some(800),
            output_tokens: 40,
            reasoning_tokens: Some(30),
        };
        let model = ScriptedModel::new([Ok(ModelResponse {
            usage: Some(usage),
            ..text("Fine.").unwrap()
        })]);
        let mut harness =
            Harness::with_script(Some("part = Box(10, 10, 2)"), FakeExecutor::new([]));
        harness
            .run_with_conversation(
                &model,
                &Conversation::new(),
                TurnInput {
                    chat: Some("Well?".into()),
                    ..Default::default()
                },
                CancelFlag::new(),
            )
            .await;
        assert!(harness.events.contains(&TurnEvent::Usage(usage)));
    }

    #[tokio::test]
    async fn printing_skill_in_a_spatial_comment_survives_history_condensation() {
        let mut history = Conversation::new();
        history.push(Message::user_chat("old detail ".repeat(1_000)));
        let model = ScriptedModel::new([text("Earlier design discussion."), text("Print advice.")]);
        let mut harness =
            Harness::with_script(Some("part = Box(10, 10, 2)"), FakeExecutor::new([]));
        let input = TurnInput {
            comments: vec![GroundedComment {
                text: "/3d-printing".into(),
                anchors: Vec::new(),
            }],
            context_window_tokens: 1_000,
            ..Default::default()
        };
        assert_eq!(
            harness
                .run_with_conversation(&model, &history, input, CancelFlag::new())
                .await,
            TurnOutcome::Answered
        );
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(!requests[0].instructions.contains("# Active skill:"));
        assert!(
            requests[1].items.iter().any(|item| matches!(item, ModelItem::Developer { text } if text.contains(skills::BUILT_IN[0].instructions)))
        );
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
    fn context_usage_counts_everything_the_request_carries() {
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("x".repeat(300)));
        let input = TurnInput {
            chat: Some("hi".into()),
            images: vec![attachment("Flange")],
            ..Default::default()
        };
        let without_images =
            RequestAssembly::new(&conversation, None, &chat("hi"), false).usage(1_000_000);
        let with_images = RequestAssembly::new(&conversation, None, &input, true).usage(1_000_000);
        let blind = RequestAssembly::new(&conversation, None, &input, false);

        assert_eq!(with_images.conversation_tokens, 75);
        assert_eq!(with_images.image_tokens, IMAGE_TOKENS);
        // The instructions and tool definitions alone are thousands of
        // tokens; the image-reading model is offered more tools.
        assert!(without_images.request_tokens > 1_000, "{without_images:?}");
        assert!(with_images.request_tokens > without_images.request_tokens);
        assert_eq!(
            with_images.used_tokens(),
            with_images.conversation_tokens + with_images.image_tokens + with_images.request_tokens
        );
        // A model that cannot read images is not sent them, and is not
        // charged for them; it is told their names.
        assert_eq!(blind.usage(1_000_000).image_tokens, 0);
        let ModelItem::User { text, images } = blind.items.last().unwrap() else {
            panic!("the request is last");
        };
        assert!(images.is_empty());
        assert!(
            text.contains("[Attached images: \"Flange\" (stored-Flange.png)]"),
            "{text}"
        );
        let ModelItem::User { images, .. } =
            RequestAssembly::new(&conversation, None, &input, true)
                .items
                .pop()
                .unwrap()
        else {
            panic!("the request is last");
        };
        assert_eq!(images, [attachment("Flange").image().unwrap()]);
    }

    fn attachment(name: &str) -> ImageAttachment {
        ImageAttachment {
            file: format!("stored-{name}.png"),
            name: name.to_string(),
            media_type: "image/png".into(),
            bytes: vec![0; 16],
        }
    }

    /// A reference library held in memory, recording what the loop did
    /// to it.
    #[derive(Default)]
    struct FakeReferences {
        files: Mutex<Vec<(String, Option<String>, ImageData)>>,
    }

    impl ReferenceSource for FakeReferences {
        fn list(&self) -> Result<Vec<ReferenceListing>, String> {
            Ok(self
                .files
                .lock()
                .unwrap()
                .iter()
                .map(|(file, description, _)| ReferenceListing {
                    file: file.clone(),
                    description: description.clone(),
                })
                .collect())
        }

        fn read(&self, file: &str) -> Result<ImageData, String> {
            self.files
                .lock()
                .unwrap()
                .iter()
                .find(|(name, _, _)| name == file)
                .map(|(_, _, image)| image.clone())
                .ok_or_else(|| format!("no {file}"))
        }

        fn keep(&self, image: &ImageData, file: &str, description: &str) -> Result<String, String> {
            let name = format!("{file}.png");
            self.files.lock().unwrap().push((
                name.clone(),
                Some(description.to_string()),
                image.clone(),
            ));
            Ok(name)
        }

        fn describe(&self, file: &str, description: &str) -> Result<(), String> {
            let mut files = self.files.lock().unwrap();
            let entry = files
                .iter_mut()
                .find(|(name, _, _)| name == file)
                .ok_or_else(|| format!("no {file}"))?;
            entry.1 = Some(description.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn the_model_lists_reads_and_keeps_reference_images() {
        let references = FakeReferences::default();
        references.files.lock().unwrap().push((
            "old.png".into(),
            None,
            ImageData {
                media_type: "image/png".into(),
                bytes: vec![9],
            },
        ));
        let list = Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: REFERENCE_IMAGES.into(),
                arguments: serde_json::json!({}),
            }],
        });
        let read = Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "c2".into(),
                name: REFERENCE_IMAGES.into(),
                arguments: serde_json::json!({"file": "old.png"}),
            }],
        });
        let keep = Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
            text: String::new(),
            tool_calls: vec![
                ToolCall {
                    id: "c3".into(),
                    name: KEEP_REFERENCE.into(),
                    arguments: serde_json::json!({
                        "source": {"attachment": {"file": "stored-Flange.png"}},
                        "file": "flange-top",
                        "description": "Top view of the flange."
                    }),
                },
                ToolCall {
                    id: "c4".into(),
                    name: KEEP_REFERENCE.into(),
                    arguments: serde_json::json!({
                        "source": {"library": {"file": "old.png"}},
                        "description": "An older sketch."
                    }),
                },
                ToolCall {
                    id: "c5".into(),
                    name: KEEP_REFERENCE.into(),
                    arguments: serde_json::json!({
                        "source": {"attachment": {"file": "stored-Nope.png"}},
                        "description": "x"
                    }),
                },
            ],
        });
        let mut model = ScriptedModel::new([list, read, keep, text("Kept it.")]);
        model.accepts_images = true;
        let project = tempfile::tempdir().unwrap();
        let mut executor = FakeExecutor::new([]);
        let mut render = NoRender;
        let mut runner = TurnRunner {
            model: &model,
            executor: &mut executor,
            docs: &FakeDocs,
            render: &mut render,
            references: &references,
            script_path: project.path().join("part.py"),
            cancel: CancelFlag::new(),
        };
        // The drawing was attached two turns ago; keeping it now must
        // still find it.
        let mut conversation = Conversation::new();
        conversation.push(
            Message::user_chat("here is the drawing").with_attachments(vec![attachment("Flange")]),
        );
        conversation.push(Message::ai_response("Noted."));
        let input = chat("keep that drawing");
        let mut finished = Vec::new();
        let outcome = runner
            .run(&conversation, &input, |event| {
                if let TurnEvent::ToolFinished {
                    call_id,
                    output,
                    failed,
                    ..
                } = event
                {
                    finished.push((call_id, output, failed));
                }
            })
            .await;
        assert_eq!(outcome, TurnOutcome::Answered);

        assert_eq!(finished.len(), 5);
        assert!(
            finished[0].1.contains("old.png: (no description yet"),
            "{}",
            finished[0].1
        );
        assert_eq!(finished[1].1, "old.png; the image follows");
        assert!(!finished[1].2);
        assert_eq!(
            finished[2].1,
            "kept as flange-top.png and described in the index"
        );
        assert_eq!(finished[3].1, "old.png is described in the index");
        assert!(finished[4].2, "an unknown attachment file fails");
        assert!(finished[4].1.contains("stored-Nope.png"));

        // The read image reached the model as a user item after the
        // tool result, like a render does.
        let requests = model.requests.lock().unwrap();
        let after_read = &requests[2].items;
        let image_item = after_read
            .iter()
            .rev()
            .find(|item| matches!(item, ModelItem::User { images, .. } if !images.is_empty()))
            .expect("the reference image follows its tool result");
        assert!(
            matches!(image_item, ModelItem::User { text, images } if text.contains("reference image") && images[0].bytes == [9])
        );

        let files = references.files.lock().unwrap();
        assert_eq!(files[0].1.as_deref(), Some("An older sketch."));
        assert_eq!(files[1].0, "flange-top.png");
        assert_eq!(files[1].2.bytes, vec![0; 16]);
    }

    #[test]
    fn history_replays_attached_images_only_to_models_that_read_them() {
        let mut conversation = Conversation::new();
        conversation
            .push(Message::user_chat("like this").with_attachments(vec![attachment("Flange")]));
        conversation.push(
            Message::spatial_comment("this face", Vec::new())
                .with_attachments(vec![attachment("Detail")]),
        );
        let seeing = history_items(conversation.messages(), true);
        let blind = history_items(conversation.messages(), false);
        for (index, name) in ["Flange", "Detail"].into_iter().enumerate() {
            let ModelItem::User { text, images } = &seeing[index] else {
                panic!("a user item");
            };
            assert_eq!(images.len(), 1);
            assert!(
                text.contains(&format!(
                    "[Attached images: \"{name}\" (stored-{name}.png)]"
                )),
                "{text}"
            );
            let ModelItem::User { text, images } = &blind[index] else {
                panic!("a user item");
            };
            assert!(images.is_empty());
            assert!(text.contains(name));
        }
    }

    #[test]
    fn a_long_script_and_an_active_skill_weigh_on_the_request() {
        let conversation = Conversation::new();
        let short =
            RequestAssembly::new(&conversation, Some("x = 1"), &chat("hi"), false).usage(100_000);
        let long_script = "y = 2\n".repeat(2_000);
        let long = RequestAssembly::new(&conversation, Some(&long_script), &chat("hi"), false)
            .usage(100_000);
        // The script sits inside a larger block, so the whole rounds once
        // where the script alone rounds once more.
        assert!(
            long.request_tokens + 1 >= short.request_tokens + estimate_tokens(&long_script),
            "{long:?} vs {short:?}"
        );
        let with_skill = RequestAssembly::new(
            &conversation,
            Some("x = 1"),
            &chat("/3d-printing hi"),
            false,
        )
        .usage(100_000);
        assert!(with_skill.request_tokens > short.request_tokens);
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
    async fn a_failed_condensation_keeps_the_turn_input_in_the_recorded_session() {
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat(format!(
            "Decision: use a 5 mm wall. {}",
            "detail ".repeat(700)
        )));
        conversation.push(Message::ai_response("The wall will be 5 mm."));
        let model = ScriptedModel::new([Err(BackendError::RequestFailed("gateway down".into()))]);
        let mut harness = Harness::with_script(Some("original"), FakeExecutor::new([]));

        let outcome = harness
            .run_with_conversation(
                &model,
                &conversation,
                TurnInput {
                    chat: Some("Open request: add a lid.".into()),
                    context_window_tokens: 1_000,
                    ..Default::default()
                },
                CancelFlag::new(),
            )
            .await;

        assert!(
            matches!(&outcome, TurnOutcome::Failed { error } if error.contains("condense")),
            "{outcome:?}"
        );
        assert_eq!(
            model.requests.lock().unwrap().len(),
            1,
            "only the condensation was asked"
        );
        let Some(TurnEvent::ModelContext { items, .. }) = harness.events.last() else {
            panic!("the turn ends by naming its model context");
        };
        let user_texts: Vec<&str> = items
            .iter()
            .filter_map(|item| match item {
                ModelItem::User { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            user_texts.iter().any(|text| text.contains("add a lid")),
            "the request the user just made is in the recorded sequence: {user_texts:?}"
        );
        assert!(user_texts.iter().any(|text| text.contains("5 mm wall")));

        // The app records that sequence as reaching the failure notice, so
        // the next turn replays it rather than rendering the chat again;
        // the open request must still reach the model.
        let mut history = conversation.clone();
        history.push(Message::user_chat("Open request: add a lid."));
        history.push(Message::notice(
            "Could not condense the conversation: gateway down",
        ));
        history.record_session(items.clone());
        let follow_up = ScriptedModel::new([text("A lid it is.")]);
        harness
            .run_with_conversation(
                &follow_up,
                &history,
                TurnInput {
                    chat: Some("Go on.".into()),
                    context_window_tokens: 128_000,
                    ..Default::default()
                },
                CancelFlag::new(),
            )
            .await;
        let requests = follow_up.requests.lock().unwrap();
        let sent: Vec<&str> = requests[0]
            .items
            .iter()
            .filter_map(|item| match item {
                ModelItem::User { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            sent.iter()
                .filter(|text| text.contains("add a lid"))
                .count(),
            1,
            "the open request is sent once: {sent:?}"
        );
        assert!(sent.last().unwrap().contains("Go on."));
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
            TurnEvent::ToolFinished { call_id, output, failed: true, .. } if call_id == "c2" && output.contains("NameError")
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
        assert_eq!(
            last.tools.len(),
            tools_for(false).len(),
            "the text-only tool set, without the image tools"
        );
        assert!(
            !last.tools.iter().any(|tool| tool.name == RENDER_VIEW),
            "no render tool for a text-only model"
        );
    }

    #[tokio::test]
    async fn a_reply_cut_off_at_the_output_limit_runs_its_whole_calls_and_the_model_continues() {
        let cut_off = Ok(ModelResponse {
            output_items: Vec::new(),
            text: "Writing the duct.".to_string(),
            reached_output_limit: true,
            usage: None,
            ..run_script("c1", "DUCT = 1", "Duct").unwrap()
        });
        let model = ScriptedModel::new([cut_off, text("The duct is done.")]);
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([Ok(())]));
        let outcome = harness
            .run(&model, chat("section the duct"), CancelFlag::new())
            .await;

        assert!(
            matches!(&outcome, TurnOutcome::Completed { source, .. } if source == "DUCT = 1"),
            "{outcome:?}"
        );
        assert_eq!(harness.executor.executed(), ["DUCT = 1"]);
        let finished = harness
            .events
            .iter()
            .position(
                |event| matches!(event, TurnEvent::ToolFinished { call_id, .. } if call_id == "c1"),
            )
            .expect("the whole call ran");
        let notice = harness
            .events
            .iter()
            .position(|event| *event == TurnEvent::Notice(OUTPUT_LIMIT_NOTICE.to_string()))
            .expect("the user is told the reply was cut off");
        assert!(finished < notice, "{:?}", harness.events);

        // The model is shown what it wrote, what its call did, and why it
        // is being asked again.
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let items = &requests[1].items;
        let tail: Vec<_> = items[items.len() - 4..].iter().collect();
        assert!(
            matches!(tail[0], ModelItem::Assistant { text } if text == "Writing the duct."),
            "{tail:?}"
        );
        assert!(matches!(tail[1], ModelItem::ToolCall(call) if call.id == "c1"));
        assert!(matches!(tail[2], ModelItem::ToolResult { call_id, .. } if call_id == "c1"));
        assert!(
            matches!(tail[3], ModelItem::User { text, .. } if text == CONTINUE_AFTER_OUTPUT_LIMIT),
            "{tail:?}"
        );
    }

    #[tokio::test]
    async fn a_reply_cut_off_at_the_output_limit_with_only_text_is_continued_too() {
        let cut_off = Ok(ModelResponse {
            output_items: Vec::new(),
            text: "The flange needs".to_string(),
            tool_calls: Vec::new(),
            reached_output_limit: true,
            usage: None,
        });
        let model = ScriptedModel::new([cut_off, text(" a thicker rim.")]);
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([]));
        let outcome = harness
            .run(&model, chat("what would you change?"), CancelFlag::new())
            .await;
        assert_eq!(outcome, TurnOutcome::Answered);
        assert_eq!(model.requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_reply_cut_off_before_it_wrote_anything_fails_the_turn_by_that_cause() {
        let cut_off = Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: true,
            usage: None,
            ..ModelResponse::default()
        });
        let model = ScriptedModel::new([cut_off]);
        let mut harness = Harness::with_script(Some("ORIGINAL = 1"), FakeExecutor::new([]));
        let outcome = harness.run(&model, chat("do it"), CancelFlag::new()).await;
        assert!(
            matches!(&outcome, TurnOutcome::Failed { error } if error.contains("output limit")),
            "{outcome:?}"
        );
        assert_eq!(model.requests.lock().unwrap().len(), 1, "not asked again");
        assert_eq!(harness.on_disk().as_deref(), Some("ORIGINAL = 1"));
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

    /// A saved run of `code` as the chat pane records it: the tool call
    /// with its arguments and a successful result.
    fn recorded_run(call_id: &str, code: &str) -> Message {
        Message::tool_calls(vec![ToolActivity {
            call_id: call_id.into(),
            tool: RUN_SCRIPT.into(),
            arguments: serde_json::json!({"code": code, "summary": "Built"}),
            output: Some("ran".into()),
            failed: false,
            started: chrono::Utc::now(),
            finished: Some(chrono::Utc::now()),
            executed_source: None,
        }])
    }

    /// The current-script block of the first request, which every turn
    /// carries whether or not a skill is active.
    fn current_script_block_shown(model: &ScriptedModel) -> String {
        model.requests.lock().unwrap()[0]
            .items
            .iter()
            .find_map(|item| match item {
                ModelItem::User { text, .. } if text.contains("<current_script>") => {
                    Some(text.clone())
                }
                _ => None,
            })
            .expect("the request carries the current script")
    }

    #[tokio::test]
    async fn every_turn_carries_the_script_on_disk_without_a_skill() {
        let model = ScriptedModel::new([text("The wall is 3 mm.")]);
        let original = "from build123d import *\nwall = 3\npart = Box(20, 10, wall)\n";
        let mut harness = Harness::with_script(Some(original), FakeExecutor::new([]));
        harness
            .run(&model, chat("How thick is the wall?"), CancelFlag::new())
            .await;
        let block = current_script_block_shown(&model);
        assert!(block.contains(original));
        assert!(block.contains("holds no run of it"));
        // The block precedes the request, so the user's words stay last.
        let requests = model.requests.lock().unwrap();
        let texts: Vec<_> = requests[0]
            .items
            .iter()
            .filter_map(|item| match item {
                ModelItem::User { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .last()
                .is_some_and(|last| *last == "How thick is the wall?")
        );
        assert!(
            texts.iter().position(|t| t.contains("<current_script>"))
                < texts.iter().position(|t| *t == "How thick is the wall?")
        );
    }

    #[tokio::test]
    async fn a_new_project_is_told_there_is_no_script_yet() {
        let model = ScriptedModel::new([text("Nothing to show.")]);
        let mut harness = Harness::with_script(None, FakeExecutor::new([]));
        harness
            .run(&model, chat("What is here?"), CancelFlag::new())
            .await;
        let context = first_request_context(&model);
        assert!(context.contains("There is no script yet"));
        assert!(!context.contains("<current_script>"));
    }

    #[tokio::test]
    async fn a_parameter_edited_in_the_panel_is_named_as_a_change_since_the_last_run() {
        let ran = "from build123d import *\nwall = 3\nwidth = 20\npart = Box(width, 10, wall)\n";
        let edited = "from build123d import *\nwall = 5\nwidth = 20\npart = Box(width, 10, wall)\n";
        let mut history = Conversation::new();
        history.push(Message::user_chat("Make a plate."));
        history.push(recorded_run("c1", ran));
        history.push(Message::ai_response("Made a plate."));
        let model = ScriptedModel::new([text("Kept the 5 mm wall.")]);
        let mut harness = Harness::with_script(Some(edited), FakeExecutor::new([]));
        harness
            .run_with_conversation(&model, &history, chat("Add a hole."), CancelFlag::new())
            .await;
        let block = current_script_block_shown(&model);
        assert!(block.contains(edited));
        assert!(block.contains("differs from the last script you ran"));
        assert!(block.contains("Parameter values that changed: wall 3 → 5"));
        let changes = block
            .lines()
            .find(|line| line.starts_with("Parameter values that changed"));
        assert_eq!(changes, Some("Parameter values that changed: wall 3 → 5"));
    }

    #[tokio::test]
    async fn a_script_unchanged_since_the_last_run_is_said_to_be_so() {
        let ran = "from build123d import *\nwall = 3\npart = Box(20, 10, wall)\n";
        let mut history = Conversation::new();
        history.push(recorded_run("c1", ran));
        let model = ScriptedModel::new([text("Still the same.")]);
        let mut harness = Harness::with_script(Some(ran), FakeExecutor::new([]));
        harness
            .run_with_conversation(&model, &history, chat("Anything new?"), CancelFlag::new())
            .await;
        let block = current_script_block_shown(&model);
        assert!(block.contains("unchanged since your last run"));
        assert!(!block.contains("Parameter values that changed"));
    }

    #[tokio::test]
    async fn a_failed_run_does_not_count_as_the_last_run() {
        let good = "from build123d import *\npart = Box(20, 10, 3)\n";
        let mut history = Conversation::new();
        history.push(recorded_run("c1", good));
        history.push(Message::tool_calls(vec![ToolActivity {
            call_id: "c2".into(),
            tool: RUN_SCRIPT.into(),
            arguments: serde_json::json!({"code": "BROKEN =", "summary": "Oops"}),
            output: Some("The script failed".into()),
            failed: true,
            started: chrono::Utc::now(),
            finished: Some(chrono::Utc::now()),
            executed_source: None,
        }]));
        let model = ScriptedModel::new([text("Same as before.")]);
        let mut harness = Harness::with_script(Some(good), FakeExecutor::new([]));
        harness
            .run_with_conversation(&model, &history, chat("Check it."), CancelFlag::new())
            .await;
        assert!(current_script_block_shown(&model).contains("unchanged since your last run"));
    }

    #[test]
    fn changed_parameter_values_ignore_derived_and_new_names() {
        let before = "wall = 3\nwidth = 20\nhalf = width / 2\n";
        let after = "wall = 4\nwidth = 20\nhalf = width / 2\ndepth = 9\n";
        assert_eq!(changed_parameter_values(before, after), ["wall 3 → 4"]);
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
        use cadmark_core::geometry::{
            EdgeId, FaceId, GeometryContext, PickedElement, TopologyElement,
        };
        use cadmark_core::ledger::LedgerValue;
        let anchor = |element: TopologyElement| GeometryContext {
            part: None,
            sketch: Default::default(),
            element: PickedElement::Solid(element),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
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
        let ModelItem::User { text, .. } = &requests[0].items[2] else {
            panic!("the user's turn follows the current script and the example library");
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
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
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
            references: &NoReferences,
            script_path: script,
            cancel: CancelFlag::new(),
        };
        let outcome = runner
            .run(&Conversation::new(), &chat("check it"), |_event| {})
            .await;
        assert_eq!(outcome, TurnOutcome::Answered);
        let requests = model.requests.lock().unwrap();
        assert!(
            requests[0]
                .tools
                .iter()
                .any(|tool| tool.name == RENDER_VIEW),
            "the render tool is offered"
        );
        let second = &requests[1];
        assert!(
            matches!(&second.items[4], ModelItem::ToolResult { call_id, .. } if call_id == "c1")
        );
        assert!(matches!(
            &second.items[5],
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
        scene.set_parts(vec![ScenePart {
            id: 0,
            name: "cube".into(),
            mesh: std::sync::Arc::new(cube_mesh()),
            bounds: Some(cadmark_renderer::camera::Bounds3 {
                min: [-0.5, -0.5, -0.5],
                max: [0.5, 0.5, 0.5],
            }),
        }]);
        scene.set_view(cadmark_renderer::camera::Camera::default(), (800, 600));
        let mut render = ViewportRender::new(scene, RenderGpu { device, queue });

        let render_call = Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
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
            references: &NoReferences,
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
        assert!(text.contains("front") && !text.contains("alone"), "{text}");
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
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
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
            references: &NoReferences,
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

    /// The test cube moved along X, as a second part beside the first.
    fn cube_mesh_at(x: f32) -> cadmark_core::mesh::TessellatedMesh {
        let mut mesh = cube_mesh();
        for vertex in &mut mesh.vertices {
            vertex.position[0] += x;
        }
        for edge in &mut mesh.edges {
            for point in &mut edge.points {
                point[0] += x;
            }
        }
        mesh
    }

    /// The share of a PNG's pixels that differ from the render background.
    fn coverage(image: &ImageData) -> f32 {
        let decoder = png::Decoder::new(std::io::Cursor::new(&image.bytes));
        let mut reader = decoder.read_info().expect("PNG header");
        let mut pixels = vec![0; reader.output_buffer_size().expect("PNG buffer size")];
        let info = reader.next_frame(&mut pixels).expect("PNG data");
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
        covered / (info.width * info.height) as f32
    }

    #[tokio::test]
    async fn a_named_part_is_rendered_alone_and_framed_to_itself() {
        // Two parts far apart. A render of the model frames both, so each
        // is a speck; a render of `peg` alone frames the peg, which then
        // fills the image. The captions say which is which.
        let (device, queue) = gpu().await;
        let scene = SceneHandle::new();
        let bounds = |x: f32| cadmark_renderer::camera::Bounds3 {
            min: [x - 0.5, -0.5, -0.5],
            max: [x + 0.5, 0.5, 0.5],
        };
        scene.set_parts(vec![
            ScenePart {
                id: 0,
                name: "base".into(),
                mesh: std::sync::Arc::new(cube_mesh()),
                bounds: Some(bounds(0.0)),
            },
            ScenePart {
                id: 1,
                name: "peg".into(),
                mesh: std::sync::Arc::new(cube_mesh_at(100.0)),
                bounds: Some(bounds(100.0)),
            },
        ]);
        scene.set_view(cadmark_renderer::camera::Camera::default(), (400, 300));
        let mut render = ViewportRender::new(scene, RenderGpu { device, queue });

        let look_twice = Ok(ModelResponse {
            output_items: Vec::new(),
            reached_output_limit: false,
            usage: None,
            text: String::new(),
            tool_calls: vec![
                ToolCall {
                    id: "c1".into(),
                    name: RENDER_VIEW.into(),
                    arguments: serde_json::json!({"view": "front"}),
                },
                ToolCall {
                    id: "c2".into(),
                    name: RENDER_VIEW.into(),
                    arguments: serde_json::json!({"view": "front", "part": "peg"}),
                },
            ],
        });
        let mut model = ScriptedModel::new([look_twice, text("Looks right.")]);
        model.accepts_images = true;
        let project = tempfile::tempdir().unwrap();
        let mut executor = FakeExecutor::new([]);
        let mut runner = TurnRunner {
            model: &model,
            executor: &mut executor,
            docs: &FakeDocs,
            render: &mut render,
            references: &NoReferences,
            script_path: project.path().join("part.py"),
            cancel: CancelFlag::new(),
        };
        let mut events = Vec::new();
        let outcome = runner
            .run(&Conversation::new(), &chat("check the peg"), |event| {
                events.push(event)
            })
            .await;
        assert_eq!(outcome, TurnOutcome::Answered);
        assert!(
            events.iter().any(|event| matches!(
                event,
                TurnEvent::Phase(phase) if phase.contains("`peg` alone") && phase.contains("front")
            )),
            "the phase line names the part, got {events:?}"
        );

        let requests = model.requests.lock().unwrap();
        let image_after = |call_id: &str| {
            let at = requests[1]
                .items
                .iter()
                .position(
                    |item| matches!(item, ModelItem::ToolResult { call_id: id, .. } if id == call_id),
                )
                .expect("the render call is answered");
            let ModelItem::User { text, images } = &requests[1].items[at + 1] else {
                panic!("the render is shown as a user item straight after its result");
            };
            (text.clone(), images.first().expect("an image").clone())
        };
        let (whole_caption, whole) = image_after("c1");
        let (peg_caption, peg) = image_after("c2");
        assert!(!whole_caption.contains("peg"), "{whole_caption}");
        assert!(peg_caption.contains("`peg` alone"), "{peg_caption}");
        let (whole, peg) = (coverage(&whole), coverage(&peg));
        assert!(
            peg > 0.2,
            "the named part fills its own frame, covered {peg:.3}"
        );
        assert!(
            whole < peg / 4.0,
            "the whole model frames both parts, so each is small: whole {whole:.3} against peg {peg:.3}"
        );
    }

    #[test]
    fn a_locked_parameter_the_script_moved_is_named_with_what_happened_to_it() {
        let before = "wall = 2  # locked: must clear the M3 head\n\
                      gap = 0.4  # locked\n\
                      height = wall * 10  # locked\n\
                      free = 1\n\
                      keep = 5  # locked\n";
        let after = "wall = 3  # locked: must clear the M3 head\n\
                     gap = 0.4\n\
                     height = wall * 12  # locked\n\
                     free = 2\n\
                     keep = 5  # locked\n\
                     new = 7  # locked\n";
        let changes = locked_parameter_changes(Some(before), after);
        assert_eq!(
            changes,
            [
                "`wall` changed 2 → 3 while locked, locked because: must clear the M3 head",
                "`gap` was unlocked (its `# locked` marker was removed)",
                "`height` changed wall * 10 → wall * 12 while locked",
            ]
        );
        // A removed one, and an unlock that also moved the value.
        let changes = locked_parameter_changes(Some(before), "gap = 0.5\nfree = 1\n");
        assert!(changes[0].starts_with("`wall` was removed"), "{changes:?}");
        assert_eq!(changes[1], "`gap` was unlocked and changed 0.4 → 0.5");
        // An augmented assignment leaves the binding as it was and still
        // moves the value; one the script already had is not a change.
        let augmented =
            "wall = 2  # locked: must clear the M3 head\nwall += 1\ngap = 0.4  # locked\n";
        assert_eq!(
            locked_parameter_changes(
                Some("wall = 2  # locked: must clear the M3 head\ngap = 0.4  # locked\n"),
                augmented
            ),
            [
                "`wall` is changed after its binding by `wall += 1` (line 2) while locked, locked because: must clear the M3 head"
            ]
        );
        assert!(locked_parameter_changes(Some(augmented), augmented).is_empty());
        // Nothing locked touched, or no script before: nothing to say.
        assert!(locked_parameter_changes(Some(before), before).is_empty());
        assert!(locked_parameter_changes(None, after).is_empty());
    }

    #[tokio::test]
    async fn a_locked_parameter_the_ai_changes_is_flagged_to_it_and_to_the_user() {
        let project = tempfile::tempdir().unwrap();
        let script_path = project.path().join("part.py");
        std::fs::write(&script_path, "wall = 2  # locked: fit\nfree = 1\n").unwrap();
        let run = |code: &str, id: &str| {
            Ok(ModelResponse {
                output_items: Vec::new(),
                reached_output_limit: false,
                usage: None,
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: id.into(),
                    name: RUN_SCRIPT.into(),
                    arguments: serde_json::json!({"code": code, "summary": "Thicker"}),
                }],
            })
        };
        let model = ScriptedModel::new([
            run("wall = 3  # locked: fit\nfree = 1\n", "c1"),
            text("Made the wall 3."),
        ]);
        let mut executor = FakeExecutor::new([]);
        let mut render = NoRender;
        let mut runner = TurnRunner {
            model: &model,
            executor: &mut executor,
            docs: &FakeDocs,
            render: &mut render,
            references: &NoReferences,
            script_path: script_path.clone(),
            cancel: CancelFlag::new(),
        };
        let mut events = Vec::new();
        let outcome = runner
            .run(&Conversation::new(), &chat("thicker wall"), |event| {
                events.push(event)
            })
            .await;
        // The user is told through the outcome, after the AI's closing
        // message, by CADmark rather than the AI.
        let TurnOutcome::Completed { locked_changes, .. } = outcome else {
            panic!("the turn completed");
        };
        assert_eq!(
            locked_changes,
            ["`wall` changed 2 → 3 while locked, locked because: fit"]
        );

        // The run result the AI read says which locked value moved and
        // what it owes: a restore, or a word in its final message.
        let run_output = events
            .iter()
            .find_map(|event| match event {
                TurnEvent::ToolFinished {
                    call_id, output, ..
                } if call_id == "c1" => Some(output.clone()),
                _ => None,
            })
            .expect("the run finished");
        assert!(
            run_output.contains("`wall` changed 2 → 3 while locked, locked because: fit"),
            "{run_output}"
        );
        assert!(run_output.contains("explicit permission"), "{run_output}");

        // A script that keeps the binding and adds `wall += 1` has moved
        // the value just the same.
        std::fs::write(&script_path, "wall = 2  # locked: fit\nfree = 1\n").unwrap();
        let model = ScriptedModel::new([
            run("wall = 2  # locked: fit\nwall += 1\nfree = 1\n", "c3"),
            text("Nudged the wall."),
        ]);
        let mut executor = FakeExecutor::new([]);
        let mut render = NoRender;
        let mut runner = TurnRunner {
            model: &model,
            executor: &mut executor,
            docs: &FakeDocs,
            render: &mut render,
            references: &NoReferences,
            script_path: script_path.clone(),
            cancel: CancelFlag::new(),
        };
        let mut events = Vec::new();
        let outcome = runner
            .run(&Conversation::new(), &chat("thicker wall"), |event| {
                events.push(event)
            })
            .await;
        let TurnOutcome::Completed { locked_changes, .. } = outcome else {
            panic!("the turn completed");
        };
        assert_eq!(
            locked_changes,
            [
                "`wall` is changed after its binding by `wall += 1` (line 2) while locked, locked because: fit"
            ]
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                TurnEvent::ToolFinished { call_id, output, .. } if call_id == "c3" && output.contains("`wall += 1`")
            )),
            "{events:?}"
        );

        // A turn that leaves every locked parameter alone raises nothing.
        std::fs::write(&script_path, "wall = 2  # locked: fit\nfree = 1\n").unwrap();
        let model = ScriptedModel::new([
            run("wall = 2  # locked: fit\nfree = 9\n", "c2"),
            text("Changed free."),
        ]);
        let mut executor = FakeExecutor::new([]);
        let mut render = NoRender;
        let mut runner = TurnRunner {
            model: &model,
            executor: &mut executor,
            docs: &FakeDocs,
            render: &mut render,
            references: &NoReferences,
            script_path,
            cancel: CancelFlag::new(),
        };
        let mut events = Vec::new();
        let outcome = runner
            .run(&Conversation::new(), &chat("more free"), |event| {
                events.push(event)
            })
            .await;
        assert!(matches!(
            outcome,
            TurnOutcome::Completed { locked_changes, .. } if locked_changes.is_empty()
        ));
        assert!(!events.iter().any(|event| matches!(
            event,
            TurnEvent::ToolFinished { output, .. } if output.contains("Locked parameters")
        )));
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
            executed_source: None,
        }]));
        conversation.push(Message::ai_response("Made a box."));
        conversation.push(Message::error_notice("not for the model"));
        conversation.push(Message::design_change(
            "Set width to 90 in the parameters panel.",
        ));
        let items = history_items(conversation.messages(), false);
        assert_eq!(items.len(), 5);
        assert!(matches!(&items[1], ModelItem::ToolCall(call) if call.id == "c1"));
        assert!(matches!(&items[2], ModelItem::ToolResult { call_id, .. } if call_id == "c1"));
        assert!(matches!(&items[3], ModelItem::Assistant { text } if text == "Made a box."));
        // What the user did outside the chat reaches the model in place,
        // so it knows when and why the script moved; a notice does not.
        assert!(matches!(
            &items[4],
            ModelItem::User { text, .. } if text == "Note from CADmark: Set width to 90 in the parameters panel."
        ));
    }

    // ── Editing, reading, and scratch snippets ────────────────────────

    /// Every tool result the turn reported, as (call, output, failed, executed).
    fn tool_results(harness: &Harness) -> Vec<(String, String, bool, Option<String>)> {
        harness
            .events
            .iter()
            .filter_map(|event| match event {
                TurnEvent::ToolFinished {
                    call_id,
                    output,
                    failed,
                    executed,
                } => Some((call_id.clone(), output.clone(), *failed, executed.clone())),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn an_edit_then_a_run_without_code_executes_the_edited_file_and_keeps_it() {
        let original = "x = 1\ny = 2\n";
        let model = ScriptedModel::new([
            edit("c1", "x = 1", "x = 10"),
            rerun("c2", "Ten"),
            text("Done."),
        ]);
        let mut harness = Harness::with_script(Some(original), FakeExecutor::new([Ok(())]));

        let outcome = harness
            .run(&model, TurnInput::default(), CancelFlag::new())
            .await;

        assert!(
            matches!(&outcome, TurnOutcome::Completed { summary, source, .. }
                if summary == "Ten" && source == "x = 10\ny = 2\n"),
            "{outcome:?}"
        );
        assert_eq!(harness.executor.executed(), vec!["x = 10\ny = 2\n"]);
        assert_eq!(
            std::fs::read_to_string(&harness.script).unwrap(),
            "x = 10\ny = 2\n"
        );
        let results = tool_results(&harness);
        assert_eq!(results[0].0, "c1");
        assert!(!results[0].2);
        assert!(results[0].1.contains("1 | x = 10"), "{}", results[0].1);
        assert_eq!(results[0].3, None, "an edit executes nothing");
        assert_eq!(results[1].0, "c2");
        assert_eq!(
            results[1].3.as_deref(),
            Some("x = 10\ny = 2\n"),
            "the run's event carries what ran"
        );
        assert!(harness.events.iter().any(|event| matches!(
            event,
            TurnEvent::Phase(phase) if phase == "editing the script"
        )));
        // The second request showed the model the edit result paired to
        // its call, before the run was asked for.
        let requests = model.requests.lock().unwrap();
        assert!(requests[1].items.iter().any(|item| matches!(
            item,
            ModelItem::ToolResult { call_id, output }
                if call_id == "c1" && output.starts_with("Replaced 1 occurrence")
        )));
    }

    #[tokio::test]
    async fn an_edit_never_run_is_discarded_and_the_turn_says_so() {
        let original = "x = 1\n";
        let model = ScriptedModel::new([edit("c1", "x = 1", "x = 10"), text("Changed x.")]);
        let mut harness = Harness::with_script(Some(original), FakeExecutor::new([]));

        let outcome = harness
            .run(&model, TurnInput::default(), CancelFlag::new())
            .await;

        assert!(
            matches!(&outcome, TurnOutcome::Failed { error } if error.contains("never ran")),
            "{outcome:?}"
        );
        assert_eq!(std::fs::read_to_string(&harness.script).unwrap(), original);
        assert!(harness.executor.executed().is_empty());
    }

    #[tokio::test]
    async fn an_edit_that_matches_nowhere_is_refused_and_the_file_is_untouched() {
        let original = "x = 1\n";
        let model = ScriptedModel::new([edit("c1", "z = 9", "z = 10"), text("Hm.")]);
        let mut harness = Harness::with_script(Some(original), FakeExecutor::new([]));

        let outcome = harness
            .run(&model, TurnInput::default(), CancelFlag::new())
            .await;

        assert_eq!(outcome, TurnOutcome::Answered);
        let results = tool_results(&harness);
        assert!(results[0].2);
        assert!(results[0].1.contains("was not found"), "{}", results[0].1);
        assert_eq!(std::fs::read_to_string(&harness.script).unwrap(), original);
    }

    #[tokio::test]
    async fn a_run_without_code_and_without_a_script_asks_for_the_file() {
        let model = ScriptedModel::new([rerun("c1", "Go"), text("Nothing to run.")]);
        let mut harness = Harness::with_script(None, FakeExecutor::new([]));

        let outcome = harness
            .run(&model, TurnInput::default(), CancelFlag::new())
            .await;

        assert!(
            matches!(&outcome, TurnOutcome::Failed { error } if error.contains("no script to run yet")),
            "{outcome:?}"
        );
        assert!(!harness.script.exists());
        assert!(harness.executor.executed().is_empty());
    }

    #[tokio::test]
    async fn read_script_shows_the_requested_lines_numbered() {
        let original = "a = 1\nb = 2\nc = 3\n";
        let model = ScriptedModel::new([
            Ok(ModelResponse {
                output_items: Vec::new(),
                reached_output_limit: false,
                usage: None,
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: READ_SCRIPT.into(),
                    arguments: serde_json::json!({"start_line": 2, "end_line": 3}),
                }],
            }),
            text("Read it."),
        ]);
        let mut harness = Harness::with_script(Some(original), FakeExecutor::new([]));

        let outcome = harness
            .run(&model, TurnInput::default(), CancelFlag::new())
            .await;

        assert_eq!(outcome, TurnOutcome::Answered);
        assert_eq!(tool_results(&harness)[0].1, "2 | b = 2\n3 | c = 3");
    }

    #[tokio::test]
    async fn run_python_runs_after_the_script_by_default_and_alone_when_standalone() {
        let model = ScriptedModel::new([
            python("c1", "part.part.volume", false),
            python("c2", "2 + 3", true),
            text("Checked."),
        ]);
        let executor = FakeExecutor::new([]).with_snippets([
            SnippetOutcome {
                printed: "faces 6\n".into(),
                value: Some("1000.0".into()),
                error: None,
            },
            SnippetOutcome {
                printed: String::new(),
                value: Some("5".into()),
                error: None,
            },
        ]);
        let mut harness = Harness::with_script(Some("x = 1\n"), executor);

        let outcome = harness
            .run(&model, TurnInput::default(), CancelFlag::new())
            .await;

        assert_eq!(outcome, TurnOutcome::Answered);
        assert_eq!(
            harness.executor.snippets_run(),
            vec![
                (Some(harness.script.clone()), "part.part.volume".to_string()),
                (None, "2 + 3".to_string()),
            ]
        );
        let results = tool_results(&harness);
        assert_eq!(results[0].1, "Printed:\nfaces 6\nValue: 1000.0");
        assert!(!results[0].2);
        assert_eq!(results[1].1, "Value: 5");
        assert!(
            harness.executor.executed().is_empty(),
            "a snippet keeps no model"
        );
    }

    #[tokio::test]
    async fn a_snippet_traceback_is_a_failed_result_the_model_reads() {
        let model = ScriptedModel::new([python("c1", "boom", false), text("I see.")]);
        let executor = FakeExecutor::new([]).with_snippets([SnippetOutcome {
            printed: String::new(),
            value: None,
            error: Some("  line 1: boom\nNameError: name 'boom' is not defined".into()),
        }]);
        let mut harness = Harness::with_script(Some("x = 1\n"), executor);

        harness
            .run(&model, TurnInput::default(), CancelFlag::new())
            .await;

        let results = tool_results(&harness);
        assert!(results[0].2);
        assert!(
            results[0].1.starts_with("Raised:\n  line 1: boom"),
            "{}",
            results[0].1
        );
        let requests = model.requests.lock().unwrap();
        assert!(requests[1].items.iter().any(|item| matches!(
            item,
            ModelItem::ToolResult { call_id, output } if call_id == "c1" && output.contains("NameError")
        )));
    }

    #[tokio::test]
    async fn a_snippet_before_any_script_needs_standalone() {
        let model = ScriptedModel::new([python("c1", "1 + 1", false), text("Ok.")]);
        let mut harness = Harness::with_script(None, FakeExecutor::new([]));

        harness
            .run(&model, TurnInput::default(), CancelFlag::new())
            .await;

        let results = tool_results(&harness);
        assert!(results[0].2);
        assert!(
            results[0].1.contains("set `standalone`"),
            "{}",
            results[0].1
        );
        assert!(harness.executor.snippets_run().is_empty());
    }

    #[tokio::test]
    async fn the_script_block_trusts_the_executed_source_of_a_run_that_carried_no_code() {
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("make x two"));
        conversation.push(Message::tool_calls(vec![ToolActivity {
            call_id: "c1".into(),
            tool: RUN_SCRIPT.into(),
            arguments: serde_json::json!({"summary": "Two"}),
            output: Some("ran".into()),
            failed: false,
            started: chrono::Utc::now(),
            finished: Some(chrono::Utc::now()),
            executed_source: Some("x = 2\n".into()),
        }]));
        let model = ScriptedModel::new([text("Still two.")]);
        let mut harness = Harness::with_script(Some("x = 2\n"), FakeExecutor::new([]));

        harness
            .run_with_conversation(
                &model,
                &conversation,
                TurnInput::default(),
                CancelFlag::new(),
            )
            .await;

        let block = current_script_block_shown(&model);
        assert!(block.contains("unchanged since your last run"), "{block}");
    }

    #[test]
    fn an_edit_replaces_the_unique_match_and_reports_the_region_numbered() {
        let source = "a = 1\nb = 2\nc = 3\nd = 4\ne = 5\nf = 6\ng = 7\n";
        let edit = apply_edit(source, "c = 3", "c = 30\nc2 = 31", false).unwrap();
        assert_eq!(
            edit.source,
            "a = 1\nb = 2\nc = 30\nc2 = 31\nd = 4\ne = 5\nf = 6\ng = 7\n"
        );
        assert_eq!(
            edit.report,
            "Replaced 1 occurrence. The script is now 8 lines.\n\
             1 | a = 1\n2 | b = 2\n3 | c = 30\n4 | c2 = 31\n5 | d = 4\n6 | e = 5"
        );
    }

    #[test]
    fn an_edit_is_refused_when_the_text_is_missing_ambiguous_or_empty() {
        let source = "x = 1\nx = 1\n";
        assert!(
            apply_edit(source, "y", "z", false)
                .unwrap_err()
                .contains("not found")
        );
        let ambiguous = apply_edit(source, "x = 1", "x = 2", false).unwrap_err();
        assert!(ambiguous.contains("occurs 2 times"), "{ambiguous}");
        assert!(
            apply_edit(source, "", "z", false)
                .unwrap_err()
                .contains("empty")
        );
        let all = apply_edit(source, "x = 1", "x = 2", true).unwrap();
        assert_eq!(all.source, "x = 2\nx = 2\n");
        assert!(
            all.report.starts_with("Replaced 2 occurrences."),
            "{}",
            all.report
        );
    }

    #[test]
    fn numbered_lines_bound_the_range_and_pad_the_numbers() {
        let source = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\n";
        assert_eq!(
            numbered_lines(source, Some(9), Some(11)),
            " 9 | i\n10 | j\n11 | k"
        );
        assert_eq!(numbered_lines(source, None, Some(2)), "1 | a\n2 | b");
        assert_eq!(numbered_lines(source, Some(10), None), "10 | j\n11 | k");
        assert!(numbered_lines(source, Some(50), None).contains("has 11 lines"));
        assert_eq!(numbered_lines("", None, None), "The script is empty.");
    }

    #[test]
    fn a_run_result_carries_what_the_script_printed() {
        let mut model = sample_model();
        model.printed = "hole 12.2\n".into();
        let text = describe_model(&model);
        assert!(text.ends_with("\nPrinted:\nhole 12.2"), "{text}");
        assert!(!describe_model(&sample_model()).contains("Printed"));
    }

    #[test]
    fn a_snippet_outcome_reads_as_its_parts() {
        assert_eq!(
            describe_snippet(&SnippetOutcome::default()),
            "Ran with no output: nothing printed and no final expression."
        );
        assert_eq!(
            describe_snippet(&SnippetOutcome {
                printed: "a\n".into(),
                value: Some("1".into()),
                error: None,
            }),
            "Printed:\na\nValue: 1"
        );
    }
}
