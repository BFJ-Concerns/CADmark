// Chat pane — the conversation, with each message shown for what it is.
//
// Four visual kinds: the user's chat (right-leaning card), spatial
// comments (accent-tinted, with a chip naming the geometry and its source
// line), the AI's replies (plain card), and notices from CADmark itself
// (quiet, or red when something failed). Applied spatial comments are
// dimmed and ticked.

use cadmark_core::geometry::GeometryContext;
use cadmark_core::ledger::LedgerValue;
use cadmark_core::message::{Conversation, Message, MessageKind};

use crate::theme;

/// The short label beside a spatial comment: which element, and which line
/// it came from when that is known.
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

/// What the chat pane is waiting on, for the indicator under the messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatActivity {
    Idle,
    /// Waiting on the AI, then executing its code.
    Generating,
    /// Executing the script on disk.
    Building,
}

/// State for the chat pane UI.
#[derive(Debug)]
pub struct ChatPane {
    /// Current text in the input field.
    pub input_text: String,
    /// What the worker is doing; input is held while it is busy.
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

    /// Render the chat pane. Returns Some(text) if the user submitted a message.
    pub fn show(&mut self, ui: &mut egui::Ui, conversation: &Conversation) -> Option<String> {
        let mut submitted = None;
        let busy = self.activity != ChatActivity::Idle;

        let input_rows = 3;
        let input_height = ui.text_style_height(&egui::TextStyle::Body) * input_rows as f32 + 16.0;
        let bottom_reserve = input_height + 34.0;
        let scroll_height = (ui.available_height() - bottom_reserve).max(60.0);

        let new_message = conversation.len() != self.seen_messages;
        self.seen_messages = conversation.len();

        egui::ScrollArea::vertical()
            .id_salt("chat_messages")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .max_height(scroll_height)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                let width = ui.available_width();
                for message in conversation.messages() {
                    show_message(ui, message, width);
                }

                match self.activity {
                    ChatActivity::Idle => {}
                    ChatActivity::Generating => {
                        activity_row(ui, "Thinking about the model\u{2026}")
                    }
                    ChatActivity::Building => activity_row(ui, "Building the model\u{2026}"),
                }
                if new_message {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                }
                ui.add_space(4.0);
            });

        ui.add_space(4.0);

        // ── Input ──────────────────────────────────────────────────
        let hint = if !self.ai_available {
            "AI is not configured. Add a cadmark.json to chat."
        } else if busy {
            "Waiting for the current step to finish\u{2026}"
        } else {
            "Describe what to build, or change\u{2026}"
        };
        let can_send = self.ai_available && !busy;

        let response = egui::Frame::new()
            .fill(theme::SUNKEN)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
            .corner_radius(egui::CornerRadius::same(theme::RADIUS + 1))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.add_sized(
                    [ui.available_width(), input_height - 12.0],
                    egui::TextEdit::multiline(&mut self.input_text)
                        .id_salt("chat_input")
                        .frame(false)
                        .hint_text(hint)
                        .desired_rows(input_rows)
                        .interactive(can_send)
                        .return_key(egui::KeyboardShortcut::new(
                            egui::Modifiers::SHIFT,
                            egui::Key::Enter,
                        )),
                )
            })
            .inner;

        if self.focus_input && can_send {
            response.request_focus();
            self.focus_input = false;
        }

        let enter_sent = response.has_focus()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Enter to send · Shift+Enter for a new line")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let has_text = !self.input_text.trim().is_empty();
                let send = ui.add_enabled(
                    can_send && has_text,
                    egui::Button::new(egui::RichText::new("Send").color(theme::TEXT_STRONG))
                        .fill(theme::ACCENT.gamma_multiply(0.55)),
                );
                if (send.clicked() || enter_sent) && can_send && has_text {
                    submitted = Some(self.input_text.trim().to_string());
                    self.input_text.clear();
                    self.focus_input = true;
                }
            });
        });

        submitted
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
                    // The card sits to the right; its text still reads
                    // from the left.
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_max_width(inner * 0.85);
                        ui.add(egui::Label::new(&message.text).wrap());
                    });
                });
            });
        }
        MessageKind::SpatialComment { context, applied } => {
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
                            theme::chip(ui, &spatial_chip(context), tint)
                                .on_hover_text(context.provenance.describe());
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

#[cfg(test)]
mod tests {
    use cadmark_core::geometry::{EdgeId, FaceId, GeometryContext, TopologyElement};
    use cadmark_core::ledger::{
        LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
    };

    use super::spatial_chip;

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
}
