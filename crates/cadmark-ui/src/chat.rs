// Chat pane — the conversation, with each message shown for what it is.
//
// The user's chat (right-leaning card), spatial comments (accent-tinted,
// with a chip per anchor naming the element and its source line), the
// AI's replies (plain card, growing as the turn streams), the steps a
// turn took — its tool calls, one collapsed line each while the turn
// runs, expandable to input and result, and its thinking, shown as it
// happens and afterwards by how long it took, with the reasoning text
// where the provider shares any — each run of steps folded into one
// counted line once the turn ends, and notices from CADmark itself
// (quiet, or red when something failed). While a turn runs, the pane
// shows what step it is on, how long it has been running, when it last
// did something, and a Cancel button.

use std::time::Instant;

use cadmark_core::geometry::{GeometryContext, PickedElement};
use cadmark_core::ledger::LedgerValue;
use cadmark_core::message::{
    ContextUsage, Conversation, ImageData, Message, MessageId, MessageKind, ToolActivity,
};
use cadmark_core::pending_comment::{
    PendingAnchor, PendingComment, PendingCommentId, PendingComments,
};

use crate::theme;
use cadmark_core::skills;

/// The short label beside a spatial comment's anchor: which element, and
/// which line it came from when that is known.
fn spatial_chip(context: &GeometryContext) -> String {
    let element = context.element.display_label();
    if let PickedElement::Sketch(_) = &context.element {
        return match context.sketch.resolved() {
            Some(source) => format!("{element} · line {}", source.source.line),
            None => format!("{element} · untraced"),
        };
    }
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
    /// The steps this turn has made: its tool-call groups and thinking
    /// records. Each shows on its own while the turn runs, and every run
    /// of them folds into one line once it ends.
    pub steps: Vec<MessageId>,
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
    /// The user asked to choose image files to attach.
    AttachImages,
    /// The user pressed paste in the input while the clipboard's text was
    /// empty, so it may hold an image; the application reads it.
    PasteImage,
    /// The user dropped these files on the window.
    DroppedFiles(Vec<std::path::PathBuf>),
}

/// An image staged with the draft: shown as a thumbnail until the
/// message is sent or the user removes it. The pane is its only owner,
/// so a thumbnail removed here is an image that will not be sent.
#[derive(Debug, Clone, PartialEq)]
pub struct StagedImage {
    /// The name the user knows it by: the file it came from, or a label
    /// for a pasted picture.
    pub name: String,
    /// The encoded bytes the message will carry.
    pub data: ImageData,
    pub thumbnail: egui::ColorImage,
}

/// State for the chat pane UI.
pub struct ChatPane {
    /// Current text in the input field.
    pub input_text: String,
    /// Images to send with the next message, in the order the strip
    /// shows them.
    pub staged_images: Vec<StagedImage>,
    /// Whether an image picker is open; the attach button waits for it.
    pub picker_pending: bool,
    /// Whether the configured model reads images. When it does not, the
    /// staged strip says so rather than letting the send look like it
    /// carried them.
    pub ai_accepts_images: bool,
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
    /// Textures for the staged thumbnails, by position; rebuilt when the
    /// staged list changes.
    staged_textures: Vec<egui::TextureHandle>,
    /// Textures for the thumbnails of sent messages, by attachment file.
    sent_textures: std::collections::HashMap<String, egui::TextureHandle>,
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
            staged_images: Vec::new(),
            picker_pending: false,
            ai_accepts_images: true,
            activity: ChatActivity::Idle,
            ai_available: true,
            seen_messages: 0,
            focus_input: true,
            staged_textures: Vec::new(),
            sent_textures: std::collections::HashMap::new(),
        }
    }

    /// Put the keyboard cursor in the input on the next frame.
    pub fn focus_input(&mut self) {
        self.focus_input = true;
    }

    /// Add an image to send with the next message.
    pub fn stage_image(&mut self, image: StagedImage) {
        self.staged_images.push(image);
        self.staged_textures.clear();
        self.focus_input = true;
    }

    /// Take an image off the strip: it will not be sent.
    pub fn remove_staged_image(&mut self, index: usize) {
        self.staged_images.remove(index);
        self.staged_textures.clear();
    }

    /// Empty the strip, handing over the images for the message being
    /// sent.
    pub fn take_staged_images(&mut self) -> Vec<StagedImage> {
        self.staged_textures.clear();
        std::mem::take(&mut self.staged_images)
    }

    /// Render the chat pane and report what the user did.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        conversation: &Conversation,
        context: ContextUsage,
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
            .show_inside(ui, |ui| {
                egui::TopBottomPanel::top("chat_context_usage")
                    .frame(egui::Frame::NONE)
                    .show_separator_line(false)
                    .show_inside(ui, |ui| show_context_usage(ui, context));
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show_inside(ui, |ui| self.show_messages(ui, conversation, pending))
                    .inner
            })
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
                let live_steps: &[MessageId] = match &self.activity {
                    ChatActivity::Turn(status) => &status.steps,
                    ChatActivity::Idle | ChatActivity::Building => &[],
                };
                let messages = conversation.messages();
                let mut index = 0;
                while index < messages.len() {
                    let message = &messages[index];
                    let live = live_steps.contains(&message.id);
                    if is_step(message) && !live {
                        // A finished turn's consecutive steps fold into one
                        // line; a running turn's stay one each.
                        let start = index;
                        while index < messages.len()
                            && is_step(&messages[index])
                            && !live_steps.contains(&messages[index].id)
                        {
                            index += 1;
                        }
                        show_step_run(ui, &messages[start..index], width);
                    } else {
                        show_message(ui, message, width, live, &mut self.sent_textures);
                        index += 1;
                    }
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

    /// The thumbnails of the images going with the next message, each
    /// with a remove control, above the text.
    fn show_staged_images(&mut self, ui: &mut egui::Ui) {
        if self.staged_textures.len() != self.staged_images.len() {
            self.staged_textures = self
                .staged_images
                .iter()
                .enumerate()
                .map(|(index, image)| {
                    ui.ctx().load_texture(
                        format!("staged/{index}/{}", image.name),
                        image.thumbnail.clone(),
                        egui::TextureOptions::LINEAR,
                    )
                })
                .collect();
        }
        let mut remove = None;
        egui::ScrollArea::horizontal()
            .id_salt("staged_images")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, image) in self.staged_images.iter().enumerate() {
                        ui.vertical(|ui| {
                            let thumbnail = ui
                                .add(
                                    egui::Image::new(&self.staged_textures[index])
                                        .max_size(egui::vec2(72.0, 54.0)),
                                )
                                .on_hover_text(&image.name);
                            ui.allocate_new_ui(
                                egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                                    thumbnail.rect.right_top() - egui::vec2(18.0, 0.0),
                                    egui::vec2(18.0, 18.0),
                                )),
                                |ui| {
                                    if ui
                                        .small_button("\u{2715}")
                                        .on_hover_text("Remove this image")
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                },
                            );
                        });
                    }
                });
            });
        if !self.ai_accepts_images {
            ui.label(
                egui::RichText::new(
                    "The configured model does not read images; enable \u{2018}The model \
                     reads images\u{2019} in Settings, or these are sent by name only.",
                )
                .small()
                .color(theme::WARNING),
            );
        }
        if let Some(index) = remove {
            self.remove_staged_image(index);
        }
        ui.add_space(4.0);
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

        // Files dropped anywhere on the window are for the message.
        let dropped: Vec<std::path::PathBuf> = ui.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if !dropped.is_empty() && self.ai_available {
            action = ChatAction::DroppedFiles(dropped);
        }

        let response = egui::Frame::new()
            .fill(theme::SUNKEN)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
            .corner_radius(egui::CornerRadius::same(theme::RADIUS + 1))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                if !self.staged_images.is_empty() {
                    self.show_staged_images(ui);
                }
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

        let enter_sent = crate::text_input::consume_submit(ui, &response);
        if self.ai_available && crate::text_input::consume_image_paste(ui, &response) {
            action = ChatAction::PasteImage;
        }

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(self.ai_available, |ui| {
                if ui
                    .add_enabled(!self.picker_pending, egui::Button::new("Attach\u{2026}"))
                    .on_hover_text("Attach PNG or JPEG images to this message. You can also paste an image, or drop files here.")
                    .clicked()
                {
                    action = ChatAction::AttachImages;
                }
                ui.menu_button("Skills", |ui| {
                    for skill in skills::BUILT_IN {
                        if ui
                            .button(format!(
                                "{}  /{} or ${}",
                                skill.title, skill.name, skill.name
                            ))
                            .on_hover_text(skill.description)
                            .clicked()
                        {
                            if skills::invoked(&self.input_text).is_none() {
                                self.input_text =
                                    format!("/{} {}", skill.name, self.input_text.trim_start());
                            }
                            self.focus_input = true;
                            ui.close_menu();
                        }
                    }
                });
            });
            let active = skills::for_turn(
                std::iter::once(self.input_text.as_str()).chain(
                    pending
                        .comments()
                        .iter()
                        .map(|comment| comment.text.as_str()),
                ),
            );
            for skill in active {
                ui.label(
                    egui::RichText::new(format!("{} · this turn", skill.title))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
        });

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Enter to send \u{b7} Shift+Enter for a new line")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let has_text = !self.input_text.trim().is_empty();
                let has_images = !self.staged_images.is_empty();
                let sending_pending = !pending.is_empty();
                // An image alone is a message: "what is this?" needs no
                // words.
                let can_submit = if sending_pending {
                    pending.can_send()
                } else {
                    has_text || has_images
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

/// How much of the context window the next request occupies, with the
/// breakdown on hover, and a warning when the request would be too large
/// with no conversation at all — which no amount of condensing helps.
fn show_context_usage(ui: &mut egui::Ui, context: ContextUsage) {
    ui.horizontal(|ui| {
        let over = context.request_alone_is_over_budget();
        let colour = if over {
            theme::WARNING
        } else {
            theme::TEXT_MUTED
        };
        ui.label(
            egui::RichText::new(format!(
                "Context: {} / {} tokens ({}%)",
                context.used_tokens(),
                context.window_tokens,
                context.percent()
            ))
            .small()
            .color(colour),
        )
        .on_hover_text(context_breakdown(context));
        if over {
            ui.label(
                egui::RichText::new(
                    "The script, instructions and images alone nearly fill the window; \
                     condensing the conversation cannot make room. Raise the context \
                     window in Settings or shorten the script.",
                )
                .small()
                .color(theme::WARNING),
            );
        }
    });
    ui.add_space(4.0);
}

/// The occupancy figure's parts, one per line, in estimated tokens.
fn context_breakdown(context: ContextUsage) -> String {
    let mut lines = vec![
        format!("Conversation: {} tokens", context.conversation_tokens),
        format!(
            "Instructions, tools, script, examples and draft: {} tokens",
            context.request_tokens
        ),
    ];
    if context.image_tokens > 0 {
        lines.push(format!(
            "Images with this message: {} tokens",
            context.image_tokens
        ));
    }
    lines.push("Estimates: four characters to a token, whatever the provider counts.".to_string());
    lines.join("\n")
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

/// The images attached to a sent message, as a row of small thumbnails
/// above its text. A thumbnail is decoded from the attachment's bytes the
/// first time it is shown and cached by file; an attachment whose file is
/// gone shows its name alone.
fn show_attachments(
    ui: &mut egui::Ui,
    message: &Message,
    textures: &mut std::collections::HashMap<String, egui::TextureHandle>,
) {
    if message.attachments.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for attachment in &message.attachments {
            let texture = match textures.get(&attachment.file) {
                Some(texture) => Some(texture.clone()),
                None => decode_thumbnail(&attachment.bytes).map(|thumbnail| {
                    let texture = ui.ctx().load_texture(
                        format!("attachment/{}", attachment.file),
                        thumbnail,
                        egui::TextureOptions::LINEAR,
                    );
                    textures.insert(attachment.file.clone(), texture.clone());
                    texture
                }),
            };
            match texture {
                Some(texture) => {
                    ui.add(egui::Image::new(&texture).max_size(egui::vec2(96.0, 72.0)))
                        .on_hover_text(&attachment.name);
                }
                None => {
                    theme::chip(ui, &attachment.name, theme::TEXT_MUTED)
                        .on_hover_text("The image file is no longer in the project");
                }
            }
        }
    });
}

/// A bounded thumbnail of an encoded image, or nothing when the bytes
/// are absent or unreadable.
fn decode_thumbnail(bytes: &[u8]) -> Option<egui::ColorImage> {
    if bytes.is_empty() {
        return None;
    }
    let decoded = image::load_from_memory(bytes).ok()?;
    let thumbnail = decoded.thumbnail(96, 96).to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [thumbnail.width() as usize, thumbnail.height() as usize],
        thumbnail.as_raw(),
    ))
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

/// Whether a message is a step of a turn — a tool-call group or a
/// thinking record — rather than something said.
fn is_step(message: &Message) -> bool {
    matches!(
        message.kind,
        MessageKind::ToolCalls(_) | MessageKind::Thinking { .. }
    )
}

/// Show one message. `live` marks a step of the running turn.
fn show_message(
    ui: &mut egui::Ui,
    message: &Message,
    width: f32,
    live: bool,
    textures: &mut std::collections::HashMap<String, egui::TextureHandle>,
) {
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
                        show_attachments(ui, message, textures);
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
                        let chips: Vec<theme::ChipEntry> = anchors
                            .iter()
                            .map(|anchor| theme::ChipEntry {
                                label: spatial_chip(anchor),
                                hover: Some(anchor.provenance.describe()),
                            })
                            .collect();
                        theme::chip_grid(ui, &chips, tint);
                        if *applied {
                            ui.label(
                                egui::RichText::new("\u{2713} applied")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        }
                        let text_colour = if *applied {
                            theme::TEXT_MUTED
                        } else {
                            theme::TEXT
                        };
                        show_attachments(ui, message, textures);
                        ui.add(
                            egui::Label::new(egui::RichText::new(&message.text).color(text_colour))
                                .wrap(),
                        );
                    });
                });
            });
        }
        MessageKind::AiResponse => {
            // A reply nothing has been written into yet is the turn's
            // placeholder, not something to show: the status line already
            // says the AI is at work.
            if message.text.is_empty() {
                return;
            }
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
        MessageKind::ConversationSummary => {
            let frame = theme::tinted_card(theme::ACCENT.gamma_multiply(0.35));
            frame.show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Earlier conversation condensed")
                        .small()
                        .strong()
                        .color(theme::TEXT_MUTED),
                );
                ui.add(egui::Label::new(&message.text).wrap());
            });
        }
        MessageKind::ToolCalls(activities) => {
            for activity in activities {
                show_tool_call(ui, message, activity, width, live);
            }
        }
        MessageKind::Thinking { finished } => show_thinking(ui, message, *finished, width, live),
        MessageKind::Notice { is_error } => show_notice(ui, &message.text, *is_error, width),
        MessageKind::DesignChange => show_notice(ui, &message.text, false, width),
    }
}

/// A note from CADmark itself: quiet, or red with a heading when something
/// went wrong.
fn show_notice(ui: &mut egui::Ui, text: &str, is_error: bool, width: f32) {
    let (tint, text_colour) = if is_error {
        (theme::ERROR, theme::TEXT)
    } else {
        (theme::TEXT_MUTED, theme::TEXT_MUTED)
    };
    let frame = theme::tinted_card(tint);
    let inner = width - frame.total_margin().sum().x;
    frame.show(ui, |ui| {
        ui.set_width(inner);
        if is_error {
            ui.label(
                egui::RichText::new("\u{26A0} Something went wrong")
                    .small()
                    .strong()
                    .color(theme::ERROR),
            );
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .color(text_colour)
                    .size(theme::SMALL_SIZE + 1.0),
            )
            .wrap(),
        );
    });
}

/// A finished turn's consecutive steps, folded into one collapsed line
/// counting the calls and totalling the thinking, which opens to the
/// same per-step lines. A run of thinking alone stands as its own lines.
fn show_step_run(ui: &mut egui::Ui, run: &[Message], width: f32) {
    let calls: Vec<&ToolActivity> = run
        .iter()
        .filter_map(|message| match &message.kind {
            MessageKind::ToolCalls(activities) => Some(activities.iter()),
            _ => None,
        })
        .flatten()
        .collect();
    if calls.is_empty() {
        for message in run {
            if let MessageKind::Thinking { finished } = &message.kind {
                show_thinking(ui, message, *finished, width, false);
            }
        }
        return;
    }
    let header = step_run_label(run, &calls);
    egui::CollapsingHeader::new(egui::RichText::new(header).small().color(theme::TEXT_MUTED))
        .id_salt(run[0].id.0)
        .default_open(false)
        .show(ui, |ui| {
            for message in run {
                match &message.kind {
                    MessageKind::ToolCalls(activities) => {
                        for activity in activities {
                            show_tool_call(ui, message, activity, width, false);
                        }
                    }
                    MessageKind::Thinking { finished } => {
                        show_thinking(ui, message, *finished, width, false);
                    }
                    _ => {}
                }
            }
        });
}

/// "3 tool calls: looked up docs, ran the script ×2 · thought for 1m 20s".
fn step_run_label(run: &[Message], calls: &[&ToolActivity]) -> String {
    let thought: i64 = run
        .iter()
        .filter_map(|message| match &message.kind {
            MessageKind::Thinking {
                finished: Some(finished),
            } => Some((*finished - message.timestamp).num_seconds().max(0)),
            _ => None,
        })
        .sum();
    let mut label = tool_group_label(calls.iter().copied());
    if thought > 0 {
        label.push_str(&format!(" \u{b7} thought for {}", duration_label(thought)));
    }
    label
}

/// The model's reasoning: "thinking…" while it goes on, "thought for 42s"
/// after. Where the provider shares the reasoning text, the line opens to
/// it; where it does not, the line stands alone.
fn show_thinking(
    ui: &mut egui::Ui,
    message: &Message,
    finished: Option<chrono::DateTime<chrono::Utc>>,
    width: f32,
    live: bool,
) {
    let label = egui::RichText::new(thinking_label(message.timestamp, finished, live))
        .small()
        .italics()
        .color(theme::TEXT_MUTED);
    if message.text.trim().is_empty() {
        ui.label(label);
        return;
    }
    egui::CollapsingHeader::new(label)
        .id_salt(message.id.0)
        .default_open(false)
        .show(ui, |ui| {
            ui.set_max_width(width);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(message.text.trim())
                        .small()
                        .color(theme::TEXT_MUTED),
                )
                .wrap(),
            );
        });
}

/// A think with no end reads as under way while its turn runs (`live`),
/// and as unfinished after.
fn thinking_label(
    started: chrono::DateTime<chrono::Utc>,
    finished: Option<chrono::DateTime<chrono::Utc>>,
    live: bool,
) -> String {
    match finished {
        None if live => "thinking\u{2026}".to_string(),
        None => "thought \u{2014} did not finish".to_string(),
        Some(finished) => format!(
            "thought for {}",
            duration_label((finished - started).num_seconds().max(0))
        ),
    }
}

/// "42s", "2m 05s", or "under a second".
fn duration_label(seconds: i64) -> String {
    match seconds {
        0 => "under a second".to_string(),
        1..60 => format!("{seconds}s"),
        _ => format!("{}m {:02}s", seconds / 60, seconds % 60),
    }
}

/// One tool call: a collapsed line saying what it did and how it went,
/// opening to its input and result.
fn show_tool_call(
    ui: &mut egui::Ui,
    message: &Message,
    activity: &ToolActivity,
    width: f32,
    live: bool,
) {
    let tint = if activity.failed {
        theme::WARNING
    } else {
        theme::TEXT_MUTED
    };
    egui::CollapsingHeader::new(
        egui::RichText::new(tool_call_label(activity, live))
            .small()
            .color(tint),
    )
    .id_salt((message.id.0, &activity.call_id))
    .default_open(false)
    .show(ui, |ui| {
        ui.set_max_width(width);
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
        let pending = if live {
            "(still running)"
        } else {
            "(the turn ended before this call finished)"
        };
        code_block(ui, activity.output.as_deref().unwrap_or(pending));
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

/// "3 tool calls: looked up docs, ran the script ×2".
fn tool_group_label<'a>(activities: impl IntoIterator<Item = &'a ToolActivity>) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    let mut calls = 0;
    for activity in activities {
        calls += 1;
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
    format!(
        "{calls} tool call{}: {}",
        if calls == 1 { "" } else { "s" },
        parts.join(", ")
    )
}

/// "ran the script — ok in 1.5 s"; a call with no result yet reads as
/// under way while its turn runs (`live`), and as unfinished after.
fn tool_call_label(activity: &ToolActivity, live: bool) -> String {
    if activity.output.is_none() && live {
        return format!("{}\u{2026}", tool_in_progress(&activity.tool));
    }
    let outcome = match (&activity.output, activity.failed) {
        (None, _) => "did not finish",
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
        "edit_script" => "edited the script",
        "read_script" => "read the script",
        "run_python" => "ran a Python snippet",
        "lookup_docs" => "looked up docs",
        "render_view" => "looked at the render",
        "reference_images" => "looked at the reference images",
        "keep_reference" => "updated the reference library",
        other => other,
    }
}

fn tool_in_progress(tool: &str) -> &str {
    match tool {
        "run_script" => "running the script",
        "edit_script" => "editing the script",
        "read_script" => "reading the script",
        "run_python" => "running a Python snippet",
        "lookup_docs" => "looking up docs",
        "render_view" => "looking at the render",
        "reference_images" => "looking at the reference images",
        "keep_reference" => "updating the reference library",
        other => other,
    }
}

/// The arguments as the user reads them: code shows as code, an edit as
/// the text taken out and the text put in, the rest as pretty JSON.
fn tool_input_text(activity: &ToolActivity) -> String {
    let text = |key: &str| activity.arguments.get(key).and_then(|value| value.as_str());
    match (text("code"), text("old_text"), text("new_text")) {
        (Some(code), _, _) => code.to_string(),
        (None, Some(old), Some(new)) => format!("--- replaced\n{old}\n+++ with\n{new}"),
        _ => serde_json::to_string_pretty(&activity.arguments).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use cadmark_core::geometry::{EdgeId, FaceId, GeometryContext, PickedElement, TopologyElement};
    use cadmark_core::ledger::{
        LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
    };
    use cadmark_core::message::ContextUsage;
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
    fn a_removed_thumbnail_is_not_among_the_images_taken_for_the_message() {
        let staged = |name: &str| StagedImage {
            name: name.to_string(),
            data: ImageData {
                media_type: "image/png".into(),
                bytes: vec![1],
            },
            thumbnail: egui::ColorImage::example(),
        };
        let mut pane = ChatPane::new();
        pane.stage_image(staged("keep"));
        pane.stage_image(staged("drop"));
        pane.stage_image(staged("also keep"));
        pane.remove_staged_image(1);
        let names: Vec<_> = pane
            .take_staged_images()
            .into_iter()
            .map(|image| image.name)
            .collect();
        assert_eq!(names, ["keep", "also keep"]);
        assert!(pane.staged_images.is_empty(), "taking empties the strip");
    }

    #[test]
    fn shift_enter_inserts_a_newline_and_only_plain_enter_sends() {
        let context = egui::Context::default();
        let mut pane = ChatPane::new();
        let pending = PendingComments::default();
        let mut frame = |events: Vec<egui::Event>| {
            let mut action = ChatAction::None;
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        action = pane.show_input(ui, &pending);
                    });
                },
            );
            (action, pane.input_text.clone())
        };
        let enter = |modifiers| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        frame(vec![]);
        frame(vec![egui::Event::Text("first line".into())]);
        let (action, text) = frame(vec![enter(egui::Modifiers::SHIFT)]);
        assert_eq!(action, ChatAction::None, "Shift+Enter must not submit");
        assert_eq!(text, "first line\n");
        frame(vec![egui::Event::Text("second line".into())]);
        let (action, text) = frame(vec![enter(egui::Modifiers::NONE)]);
        assert_eq!(action, ChatAction::Send("first line\nsecond line".into()));
        assert!(text.is_empty());
    }

    #[test]
    fn chip_names_element_and_known_lines() {
        let resolved = GeometryContext {
            sketch: Default::default(),
            element: PickedElement::Solid(TopologyElement::Face(FaceId(1))),
            provenance: LedgerValue::Resolved(entry(7)),
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
        };
        assert_eq!(spatial_chip(&resolved), "face 1 · line 7");
        let ambiguous = GeometryContext {
            sketch: Default::default(),
            element: PickedElement::Solid(TopologyElement::Edge(EdgeId(2))),
            provenance: LedgerValue::Ambiguous(vec![entry(3), entry(9)]),
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
        };
        assert_eq!(spatial_chip(&ambiguous), "edge 2 · lines 3/9");
        let drawn = GeometryContext {
            sketch: cadmark_core::sketch_lineage::SketchLineage::Resolved(
                cadmark_core::sketch_lineage::SketchSource {
                    source: cadmark_core::ledger::SourceRef {
                        line: 4,
                        code: "Rectangle(20, 10)".into(),
                    },
                    object: "Rectangle".into(),
                },
            ),
            element: PickedElement::Sketch(cadmark_core::geometry::SketchElement {
                kind: cadmark_core::geometry::SketchElementKind::Region,
                index: 0,
            }),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
        };
        assert_eq!(spatial_chip(&drawn), "sketch region 0 · line 4");
        let untraced = GeometryContext {
            sketch: Default::default(),
            element: PickedElement::Solid(TopologyElement::Edge(EdgeId(2))),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
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
            executed_source: None,
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
            "3 tool calls: looked up docs, ran the script \u{00d7}2"
        );
        assert_eq!(
            tool_group_label(&activities[..1]),
            "1 tool call: looked up docs"
        );
        assert_eq!(
            tool_call_label(&activities[1], true),
            "ran the script \u{2014} failed in 1.5 s"
        );
        let unfinished = activity("run_script", None, false);
        assert_eq!(
            tool_call_label(&unfinished, true),
            "running the script\u{2026}"
        );
        assert_eq!(
            tool_call_label(&unfinished, false),
            "ran the script \u{2014} did not finish",
            "a call the turn never finished must not read as still running"
        );
        assert_eq!(tool_input_text(&activities[1]), "X = 1");
    }

    /// Every piece of text one frame of the pane draws.
    fn drawn_text(pane: &mut ChatPane, conversation: &Conversation) -> Vec<String> {
        let context = egui::Context::default();
        let mut pending = PendingComments::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 900.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    pane.show(
                        ui,
                        conversation,
                        ContextUsage {
                            conversation_tokens: 0,
                            image_tokens: 0,
                            request_tokens: 0,
                            window_tokens: 128_000,
                        },
                        &mut pending,
                    );
                });
            },
        );
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// A thinking record that began `seconds_ago` and, unless still going,
    /// ended now, carrying `text` of the reasoning.
    fn thinking(text: &str, seconds_ago: i64, finished: bool) -> Message {
        let mut message = Message::thinking();
        message.timestamp = chrono::Utc::now() - chrono::Duration::seconds(seconds_ago);
        message.text = text.to_string();
        if finished {
            message.kind = MessageKind::Thinking {
                finished: Some(chrono::Utc::now()),
            };
        }
        message
    }

    #[test]
    fn a_running_turn_shows_each_step_and_a_finished_one_folds_a_run_of_them_under_a_count() {
        let mut conversation = Conversation::new();
        conversation.push(Message::user_chat("Make a flange"));
        let thought = conversation.push(thinking("", 42, true));
        conversation.push(Message::ai_response("Checking the fillet API first."));
        let group = conversation.push(Message::tool_calls(vec![
            activity("lookup_docs", Some("docs"), false),
            activity("run_script", None, false),
        ]));
        let shared = conversation.push(thinking("Weigh the options.", 30, true));
        let second = conversation.push(Message::tool_calls(vec![activity(
            "run_script",
            Some("ok"),
            false,
        )]));
        conversation.push(Message::ai_response("Running it now."));
        let thinking_now = conversation.push(thinking("", 3, false));
        conversation.push(Message::ai_response(""));
        let mut pane = ChatPane::new();

        let now = Instant::now();
        pane.activity = ChatActivity::Turn(TurnStatus {
            phase: "thinking".into(),
            started: now,
            last_event: now,
            steps: vec![thought, group, shared, second, thinking_now],
        });
        let running = drawn_text(&mut pane, &conversation);
        let shows = |text: &[String], wanted: &str| text.iter().any(|drawn| drawn == wanted);
        assert!(
            shows(&running, "looked up docs \u{2014} ok in 1.5 s"),
            "{running:?}"
        );
        assert!(shows(&running, "running the script\u{2026}"), "{running:?}");
        assert!(shows(&running, "thought for 42s"), "{running:?}");
        assert!(shows(&running, "thought for 30s"), "{running:?}");
        assert!(
            shows(&running, "thinking\u{2026}"),
            "the think under way shows as such: {running:?}"
        );
        assert!(
            !running.iter().any(|drawn| drawn.contains("tool calls")),
            "a running turn's steps are not folded: {running:?}"
        );
        assert!(
            !shows(&running, "X = 1") && !shows(&running, "Weigh the options."),
            "each step starts collapsed: {running:?}"
        );
        assert!(shows(&running, "Checking the fillet API first."));
        assert!(shows(&running, "Running it now."));
        assert_eq!(
            running.iter().filter(|drawn| *drawn == "CADmark").count(),
            2,
            "a reply still empty draws no card: {running:?}"
        );

        pane.activity = ChatActivity::Idle;
        let finished = drawn_text(&mut pane, &conversation);
        assert!(
            shows(&finished, "thought for 42s"),
            "a think with no calls beside it stands alone: {finished:?}"
        );
        assert!(
            shows(
                &finished,
                "3 tool calls: looked up docs, ran the script \u{00d7}2 \u{b7} thought for 30s"
            ),
            "calls and the thinking between them fold as one run: {finished:?}"
        );
        assert!(
            !finished
                .iter()
                .any(|drawn| drawn.starts_with("looked up docs") || drawn == "thought for 30s"),
            "the steps fold under the count: {finished:?}"
        );
        assert!(
            shows(&finished, "Checking the fillet API first.")
                && shows(&finished, "Running it now."),
            "what the AI wrote stays shown: {finished:?}"
        );
        assert!(
            shows(&finished, "thought \u{2014} did not finish"),
            "a think with no end must not read as still going after the turn: {finished:?}"
        );
    }

    #[test]
    fn thinking_reads_by_how_long_it_took() {
        let started = chrono::Utc::now();
        assert_eq!(thinking_label(started, None, true), "thinking\u{2026}");
        assert_eq!(
            thinking_label(started, None, false),
            "thought \u{2014} did not finish"
        );
        let after = |seconds: i64| Some(started + chrono::Duration::seconds(seconds));
        assert_eq!(
            thinking_label(started, after(0), false),
            "thought for under a second"
        );
        assert_eq!(thinking_label(started, after(42), false), "thought for 42s");
        assert_eq!(
            thinking_label(started, after(125), false),
            "thought for 2m 05s"
        );
    }

    #[test]
    fn the_status_line_names_the_phase_and_flags_a_quiet_stream() {
        let now = Instant::now();
        let busy = TurnStatus {
            phase: "thinking".into(),
            started: now,
            last_event: now,
            steps: Vec::new(),
        };
        assert_eq!(turn_status_line(&busy), "thinking · 0s");
        let quiet = TurnStatus {
            phase: "thinking".into(),
            started: now - std::time::Duration::from_secs(75),
            last_event: now - std::time::Duration::from_secs(9),
            steps: Vec::new(),
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
                action = pane.show(
                    ui,
                    &conversation,
                    cadmark_core::message::ContextUsage {
                        conversation_tokens: 0,
                        image_tokens: 0,
                        request_tokens: 0,
                        window_tokens: 128_000,
                    },
                    &mut pending,
                );
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
            sketch: Default::default(),
            element: PickedElement::Solid(TopologyElement::Face(FaceId(face))),
            provenance: LedgerValue::Untraced,
            identification: Default::default(),
            source_context: String::new(),
            neighbours: Vec::new(),
            chosen_candidate: None,
        }
    }

    #[test]
    fn the_context_breakdown_names_each_part_and_the_images_only_when_present() {
        let usage = ContextUsage {
            conversation_tokens: 120,
            image_tokens: 0,
            request_tokens: 4_500,
            window_tokens: 128_000,
        };
        let text = context_breakdown(usage);
        assert!(text.contains("Conversation: 120 tokens"), "{text}");
        assert!(
            text.contains("script, examples and draft: 4500 tokens"),
            "{text}"
        );
        assert!(!text.contains("Images with this message"), "{text}");
        let with_images = context_breakdown(ContextUsage {
            image_tokens: 765,
            ..usage
        });
        assert!(
            with_images.contains("Images with this message: 765 tokens"),
            "{with_images}"
        );
    }
}
