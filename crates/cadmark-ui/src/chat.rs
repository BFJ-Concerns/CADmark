// Chat pane — the conversation, with each message shown for what it is.
//
// The user's chat (right-leaning card), spatial comments (accent-tinted,
// with a chip per anchor naming the element and its source line), the
// AI's replies (plain card, growing as the turn streams), the tool calls
// a turn made (one collapsed group, expandable to each call's input and
// result), and notices from CADmark itself (quiet, or red when something
// failed). While a turn runs, the pane shows what step it is on, how long
// it has been running, when it last did something, and a Cancel button.

use std::time::Instant;

use cadmark_core::geometry::GeometryContext;
use cadmark_core::ledger::LedgerValue;
use cadmark_core::message::{Conversation, Message, MessageKind, ToolActivity};
use cadmark_core::pending_comment::{
    PendingAnchor, PendingComment, PendingCommentId, PendingComments,
};

use crate::theme;

/// The short label beside a spatial comment's anchor: which element, and
/// which line it came from when that is known.
fn spatial_chip(context: &GeometryContext) -> String {
    let element = context.element.display_label();
    match &context.provenance {
        LedgerValue::Resolved(entry) => format!("{element} · line {}", entry.source.line),
        LedgerValue::Ambiguous(candidates) => format!(
            "{element} · lines {}",
            candidates
                .iter()
                .map(|candidate| candidate.source.line.to_string())
                .collect::<Vec<_>>()
                .join("/")
        ),
        LedgerValue::Untraced => format!("{element} · untraced"),
    }
}

/// What the pane is waiting on, for the indicator under the messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatActivity {
    Idle,
    /// An AI turn is running.
    Turn(TurnStatus),
    /// Executing the script on disk.
    Building,
}

/// The running turn as the user watches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStatus {
    /// What the turn is doing right now.
    pub phase: String,
    pub started: Instant,
    /// When the turn last produced an event; a quiet stream shows as quiet.
    pub last_event: Instant,
}

/// What the user did in the pane this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatAction {
    None,
    /// The user submitted this text.
    Send(String),
    /// Submit every pending spatial comment with optional chat text as one turn.
    SendPending {
        chat: Option<String>,
    },
    /// Remove one unsent spatial-comment card.
    RemovePending(PendingCommentId),
    /// The user asked for the running turn to stop.
    Cancel,
}

/// State for the chat pane UI.
#[derive(Debug)]
pub struct ChatPane {
    /// Current text in the input field.
    pub input_text: String,
    /// What the worker is doing.
    pub activity: ChatActivity,
    /// Whether AI is configured. Without it the input explains why rather
    /// than sending a message nowhere.
    pub ai_available: bool,
    /// Number of messages seen on the last frame, so a new one scrolls the
    /// list to the bottom exactly once.
    seen_messages: usize,
    /// Focus the input on the next frame.
    focus_input: bool,
}

impl Default for ChatPane {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatPane {
    pub fn new() -> Self {
        Self {
            input_text: String::new(),
            activity: ChatActivity::Idle,
            ai_available: true,
            seen_messages: 0,
            focus_input: true,
        }
    }

    /// Put the keyboard cursor in the input on the next frame.
    pub fn focus_input(&mut self) {
        self.focus_input = true;
    }

    /// Render the chat pane and report what the user did.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        conversation: &Conversation,
        pending: &mut PendingComments,
    ) -> ChatAction {
        // The input sits in a bottom panel so it is laid out first and the
        // messages take whatever height remains: however tall the input
        // grows, the send row below it stays on screen.
        let action = egui::TopBottomPanel::bottom("chat_input_panel")
            .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                left: 0,
                right: 0,
                top: 4,
                bottom: 0,
            }))
            .show_separator_line(false)
            .show_inside(ui, |ui| self.show_input(ui, pending))
            .inner;

        let remove = egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| self.show_messages(ui, conversation, pending))
            .inner;

        remove.map_or(action, ChatAction::RemovePending)
    }

    fn show_messages(
        &mut self,
        ui: &mut egui::Ui,
        conversation: &Conversation,
        pending: &mut PendingComments,
    ) -> Option<PendingCommentId> {
        let new_message = conversation.len() != self.seen_messages;
        self.seen_messages = conversation.len();

        egui::ScrollArea::vertical()
            .id_salt("chat_messages")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                let width = ui.available_width();
                for message in conversation.messages() {
                    show_message(ui, message, width);
                }
                // Pending cards follow history so bottom sticking keeps a
                // newly staged card in view rather than hiding it above it.
                let mut remove = None;
                for comment in pending.comments_mut() {
                    if show_pending_comment(ui, comment, width) {
                        remove = Some(comment.id);
                    }
                }

                match &self.activity {
                    ChatActivity::Idle => {}
                    ChatActivity::Turn(status) => activity_row(ui, &turn_status_line(status)),
                    ChatActivity::Building => activity_row(ui, "Building the model\u{2026}"),
                }
                if new_message {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                }
                ui.add_space(4.0);
                remove
            })
            .inner
    }

    /// The input box and the send row under it. The box grows with its
    /// text up to a cap, then scrolls inside itself. Typing is never
    /// blocked by a running turn: the text waits for the turn to end.
    fn show_input(&mut self, ui: &mut egui::Ui, pending: &PendingComments) -> ChatAction {
        let mut action = ChatAction::None;
        let turn_running = matches!(self.activity, ChatActivity::Turn(_));
        let busy = self.activity != ChatActivity::Idle;

        let hint = if !self.ai_available {
            "AI is not configured. Open Settings to add a provider."
        } else if turn_running {
            "The AI is working; your next message waits for it\u{2026}"
        } else {
            "Describe what to build, or change\u{2026}"
        };
        let can_send = self.ai_available && !busy;

        let row_height = ui.text_style_height(&egui::TextStyle::Body);
        let min_rows = 3;
        let max_input_height = row_height * 10.0;

        let response = egui::Frame::new()
            .fill(theme::SUNKEN)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
            .corner_radius(egui::CornerRadius::same(theme::RADIUS + 1))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("chat_input_scroll")
                    .max_height(max_input_height)
                    .auto_shrink([false, true])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.input_text)
                                .id_salt("chat_input")
                                .frame(false)
                                .hint_text(hint)
                                .desired_rows(min_rows)
                                .desired_width(f32::INFINITY)
                                .interactive(self.ai_available)
                                .return_key(egui::KeyboardShortcut::new(
                                    egui::Modifiers::SHIFT,
                                    egui::Key::Enter,
                                )),
                        )
                    })
                    .inner
            })
            .inner;

        if self.focus_input && self.ai_available {
            response.request_focus();
            self.focus_input = false;
        }

        let enter_sent = response.has_focus()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Enter to send \u{b7} Shift+Enter for a new line")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let has_text = !self.input_text.trim().is_empty();
                let sending_pending = !pending.is_empty();
                let can_submit = if sending_pending {
                    pending.can_send()
                } else {
                    has_text
                };
                if turn_running
                    && ui
                        .add(egui::Button::new(
                            egui::RichText::new("Cancel").color(theme::ERROR),
                        ))
                        .on_hover_text("Stop the AI and put the model back as it was")
                        .clicked()
                {
                    action = ChatAction::Cancel;
                }
                let send = ui.add_enabled(
                    can_send && can_submit,
                    egui::Button::new(
                        egui::RichText::new(if sending_pending {
                            format!("Send {} comments", pending.comments().len())
                        } else {
                            "Send".to_string()
                        })
                        .color(theme::TEXT_STRONG),
                    )
                    .fill(theme::ACCENT.gamma_multiply(0.55)),
                );
                if (send.clicked() || enter_sent) && can_send && can_submit {
                    action = if sending_pending {
                        ChatAction::SendPending {
                            chat: has_text.then(|| self.input_text.trim().to_string()),
                        }
                    } else {
                        ChatAction::Send(self.input_text.trim().to_string())
                    };
                    self.input_text.clear();
                    self.focus_input = true;
                }
            });
        });

        action
    }
}

fn marker_colour(comment: &PendingComment) -> egui::Color32 {
    let [red, green, blue, alpha] = comment.marker_colour();
    egui::Color32::from_rgba_unmultiplied(
        (red * 255.0) as u8,
        (green * 255.0) as u8,
        (blue * 255.0) as u8,
        (alpha * 255.0) as u8,
    )
}

fn pending_anchor_label(anchor: &PendingAnchor) -> String {
    match anchor {
        PendingAnchor::Live(context) => spatial_chip(context),
        PendingAnchor::Lost { element } => format!("{} · lost", element.display_label()),
    }
}

fn pending_comment_text_id(id: PendingCommentId) -> egui::Id {
    egui::Id::new(("pending_comment", id.0))
}

/// Render one editable unsent card. Returns true when the user removes it.
fn show_pending_comment(ui: &mut egui::Ui, comment: &mut PendingComment, width: f32) -> bool {
    let colour = marker_colour(comment);
    let frame = theme::tinted_card(colour);
    let inner = width - frame.total_margin().sum().x;
    let mut remove = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
        frame.show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_max_width(inner * 0.85);
                ui.horizontal_wrapped(|ui| {
                    theme::chip(ui, &format!("Comment {}", comment.marker_number), colour);
                    for anchor in &comment.anchors {
                        let label = pending_anchor_label(anchor);
                        ui.label(egui::RichText::new(label).small().color(theme::TEXT_MUTED));
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut comment.text)
                        .id(pending_comment_text_id(comment.id))
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    if comment
                        .anchors
                        .iter()
                        .any(|anchor| matches!(anchor, PendingAnchor::Lost { .. }))
                    {
                        ui.label(
                            egui::RichText::new("Re-point lost geometry before sending")
                                .small()
                                .color(theme::ERROR),
                        );
                    }
                    if ui
                        .button(egui::RichText::new("Remove").color(theme::ERROR))
                        .clicked()
                    {
                        remove = true;
                    }
                });
            });
        });
    });
    remove
}

/// "looking up build123d docs · 1m 12s · last event 3s ago".
fn turn_status_line(status: &TurnStatus) -> String {
    let quiet = status.last_event.elapsed().as_secs();
    let mut line = format!("{} · {}", status.phase, elapsed(status.started));
    if quiet >= 5 {
        line.push_str(&format!(" · quiet for {quiet}s"));
    }
    line
}

fn elapsed(since: Instant) -> String {
    let seconds = since.elapsed().as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

fn activity_row(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.add(egui::Spinner::new().size(14.0).color(theme::ACCENT));
        ui.label(egui::RichText::new(text).italics().color(theme::TEXT_MUTED));
    });
}

fn show_message(ui: &mut egui::Ui, message: &Message, width: f32) {
    // A card's content must leave room for its own margins and border, or
    // the card outgrows the pane and the pane grows to match, every frame.
    let card_inner = |frame: &egui::Frame| width - frame.total_margin().sum().x;
    match &message.kind {
        MessageKind::UserChat => {
            // The user's own words sit to the right, slightly narrower, so
            // the two voices are told apart by position as well as colour.
            let frame = theme::tinted_card(theme::TEXT_MUTED);
            let inner = card_inner(&frame);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                frame.show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_max_width(inner * 0.85);
                        ui.add(egui::Label::new(&message.text).wrap());
                    });
                });
            });
        }
        MessageKind::SpatialComment { anchors, applied } => {
            let tint = if *applied {
                theme::SPATIAL.gamma_multiply(0.55)
            } else {
                theme::SPATIAL
            };
            let frame = theme::tinted_card(tint);
            let inner = card_inner(&frame);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                frame.show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_max_width(inner * 0.85);
                        ui.set_min_width(inner * 0.55);
                        ui.horizontal_wrapped(|ui| {
                            for anchor in anchors {
                                theme::chip(ui, &spatial_chip(anchor), tint)
                                    .on_hover_text(anchor.provenance.describe());
                            }
                            if *applied {
                                ui.label(
                                    egui::RichText::new("\u{2713} applied")
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                );
                            }
                        });
                        let text_colour = if *applied {
                            theme::TEXT_MUTED
                        } else {
                            theme::TEXT
                        };
                        ui.add(
                            egui::Label::new(egui::RichText::new(&message.text).color(text_colour))
                                .wrap(),
                        );
                    });
                });
            });
        }
        MessageKind::AiResponse => {
            let frame = theme::card();
            let inner = card_inner(&frame);
            frame.show(ui, |ui| {
                ui.set_width(inner);
                ui.label(
                    egui::RichText::new("CADmark")
                        .small()
                        .strong()
                        .color(theme::AI),
                );
                ui.add(egui::Label::new(&message.text).wrap());
            });
        }
        MessageKind::ToolCalls(activities) => show_tool_calls(ui, message, activities, width),
        MessageKind::Notice { is_error } => {
            let (tint, text_colour) = if *is_error {
                (theme::ERROR, theme::TEXT)
            } else {
                (theme::TEXT_MUTED, theme::TEXT_MUTED)
            };
            let frame = theme::tinted_card(tint);
            let inner = card_inner(&frame);
            frame.show(ui, |ui| {
                ui.set_width(inner);
                if *is_error {
                    ui.label(
                        egui::RichText::new("\u{26A0} Something went wrong")
                            .small()
                            .strong()
                            .color(theme::ERROR),
                    );
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&message.text)
                            .color(text_colour)
                            .size(theme::SMALL_SIZE + 1.0),
                    )
                    .wrap(),
                );
            });
        }
    }
}

/// A run of tool calls: one collapsed line naming what was done, opening
/// to each call's input and result.
fn show_tool_calls(ui: &mut egui::Ui, message: &Message, activities: &[ToolActivity], width: f32) {
    let header = tool_group_label(activities);
    egui::CollapsingHeader::new(egui::RichText::new(header).small().color(theme::TEXT_MUTED))
        .id_salt(message.id.0)
        .default_open(false)
        .show(ui, |ui| {
            ui.set_max_width(width);
            for activity in activities {
                let tint = if activity.failed {
                    theme::WARNING
                } else {
                    theme::TEXT_MUTED
                };
                egui::CollapsingHeader::new(
                    egui::RichText::new(tool_call_label(activity))
                        .small()
                        .color(tint),
                )
                .id_salt((message.id.0, &activity.call_id))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("Input")
                            .small()
                            .strong()
                            .color(theme::TEXT_MUTED),
                    );
                    code_block(ui, &tool_input_text(activity));
                    ui.label(
                        egui::RichText::new("Result")
                            .small()
                            .strong()
                            .color(theme::TEXT_MUTED),
                    );
                    code_block(ui, activity.output.as_deref().unwrap_or("(still running)"));
                });
            }
        });
}

fn code_block(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::SUNKEN)
        .corner_radius(egui::CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(text).monospace().color(theme::TEXT)).wrap(),
            );
        });
}

/// "3 steps: looked up docs, ran the script ×2".
fn tool_group_label(activities: &[ToolActivity]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for activity in activities {
        match counts.iter_mut().find(|(tool, _)| *tool == activity.tool) {
            Some((_, count)) => *count += 1,
            None => counts.push((&activity.tool, 1)),
        }
    }
    let parts: Vec<String> = counts
        .into_iter()
        .map(|(tool, count)| {
            let verb = tool_verb(tool);
            if count > 1 {
                format!("{verb} \u{00d7}{count}")
            } else {
                verb.to_string()
            }
        })
        .collect();
    let steps = activities.len();
    format!(
        "{steps} step{}: {}",
        if steps == 1 { "" } else { "s" },
        parts.join(", ")
    )
}

fn tool_call_label(activity: &ToolActivity) -> String {
    let outcome = match (&activity.output, activity.failed) {
        (None, _) => "running",
        (Some(_), true) => "failed",
        (Some(_), false) => "ok",
    };
    let took = match (activity.finished, activity.started) {
        (Some(finished), started) => {
            let ms = (finished - started).num_milliseconds().max(0);
            if ms < 1000 {
                format!("{ms} ms")
            } else {
                format!("{:.1} s", ms as f64 / 1000.0)
            }
        }
        (None, _) => String::new(),
    };
    let mut label = format!("{} \u{2014} {outcome}", tool_verb(&activity.tool));
    if !took.is_empty() {
        label.push_str(&format!(" in {took}"));
    }
    label
}

fn tool_verb(tool: &str) -> &str {
    match tool {
        "run_script" => "ran the script",
        "lookup_docs" => "looked up docs",
        "render_view" => "looked at the render",
        other => other,
    }
}

/// The arguments as the user reads them: a script shows as its code, the
/// rest as pretty JSON.
fn tool_input_text(activity: &ToolActivity) -> String {
    match activity
        .arguments
        .get("code")
        .and_then(|code| code.as_str())
    {
        Some(code) => code.to_string(),
        None => serde_json::to_string_pretty(&activity.arguments).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use cadmark_core::geometry::{EdgeId, FaceId, GeometryContext, TopologyElement};
    use cadmark_core::ledger::{
        LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
    };
    use cadmark_core::message::{Conversation, ToolActivity};

    use super::*;

    fn entry(line: u32) -> ProvenanceEntry {
        ProvenanceEntry {
            source: SourceRef {
                line,
                code: String::new(),
            },
            operation: SemanticOperation::Box,
            operation_id: u64::from(line),
            relation: ProvenanceRelation::Generated,
        }
    }

    #[test]
    fn chip_names_element_and_known_lines() {
        let resolved = GeometryContext {
            element: TopologyElement::Face(FaceId(1)),
            provenance: LedgerValue::Resolved(entry(7)),
            identification: Default::default(),
        };
        assert_eq!(spatial_chip(&resolved), "face 1 · line 7");
        let ambiguous = GeometryContext {
            element: TopologyElement::Edge(EdgeId(2)),
            provenance: LedgerValue::Ambiguous(vec![entry(3), entry(9)]),
            identification: Default::default(),
        };
        assert_eq!(spatial_chip(&ambiguous), "edge 2 · lines 3/9");
        let untraced = GeometryContext {
            element: TopologyElement::Edge(EdgeId(2)),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
        };
        assert_eq!(spatial_chip(&untraced), "edge 2 · untraced");
    }

    fn activity(tool: &str, output: Option<&str>, failed: bool) -> ToolActivity {
        let started = chrono::Utc::now();
        ToolActivity {
            call_id: format!("call-{tool}"),
            tool: tool.into(),
            arguments: serde_json::json!({"code": "X = 1"}),
            output: output.map(str::to_string),
            failed,
            started,
            finished: output.map(|_| started + chrono::Duration::milliseconds(1500)),
        }
    }

    #[test]
    fn a_tool_group_reads_as_a_count_of_what_was_done() {
        let activities = vec![
            activity("lookup_docs", Some("docs"), false),
            activity("run_script", Some("boom"), true),
            activity("run_script", Some("ok"), false),
        ];
        assert_eq!(
            tool_group_label(&activities),
            "3 steps: looked up docs, ran the script \u{00d7}2"
        );
        assert_eq!(
            tool_call_label(&activities[1]),
            "ran the script \u{2014} failed in 1.5 s"
        );
        assert_eq!(
            tool_call_label(&activity("run_script", None, false)),
            "ran the script \u{2014} running"
        );
        assert_eq!(tool_input_text(&activities[1]), "X = 1");
    }

    #[test]
    fn the_status_line_names_the_phase_and_flags_a_quiet_stream() {
        let now = Instant::now();
        let busy = TurnStatus {
            phase: "thinking".into(),
            started: now,
            last_event: now,
        };
        assert_eq!(turn_status_line(&busy), "thinking · 0s");
        let quiet = TurnStatus {
            phase: "thinking".into(),
            started: now - std::time::Duration::from_secs(75),
            last_event: now - std::time::Duration::from_secs(9),
        };
        assert_eq!(turn_status_line(&quiet), "thinking · 1m 15s · quiet for 9s");
    }

    #[test]
    fn editing_a_surviving_card_through_its_text_edit_preserves_marker_pairing() {
        let mut pending = PendingComments::default();
        let first = pending.add("round this".into(), vec![untraced_face(1)]);
        let removed = pending.add("remove this comment".into(), vec![untraced_face(2)]);
        let third = pending.add("chamfer this".into(), vec![untraced_face(3)]);
        pending.remove(removed).expect("the middle card exists");
        let third_colour = pending.comments()[1].marker_colour();

        let context = egui::Context::default();
        context.memory_mut(|memory| memory.request_focus(pending_comment_text_id(third)));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events: vec![egui::Event::Text(" more".into())],
            ..Default::default()
        };
        let mut pane = ChatPane::new();
        pane.focus_input = false;
        let conversation = Conversation::new();
        let mut action = ChatAction::None;
        let _ = context.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                action = pane.show(ui, &conversation, &mut pending);
            });
        });

        assert_eq!(action, ChatAction::None);
        assert_eq!(
            pending
                .comments()
                .iter()
                .map(|comment| (comment.id, comment.marker_number, comment.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(first, 1, "round this"), (third, 3, "chamfer this more")]
        );
        assert_eq!(
            pending.comments()[1].marker_colour(),
            third_colour,
            "editing through the card must not replace its marker pairing"
        );
    }

    fn untraced_face(face: u32) -> GeometryContext {
        GeometryContext {
            element: TopologyElement::Face(FaceId(face)),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
        }
    }
}
