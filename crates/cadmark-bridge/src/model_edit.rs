//! Provider-independent model-edit prompting and response parsing.

use std::future::Future;
use std::pin::Pin;

use cadmark_core::ledger::LedgerValue;

use crate::backend::{AiBackend, AiResponse, BackendError};
use crate::context::AiRequest;
use crate::openai_compatible::OpenAiCompatibleClient;

const SYSTEM_PROMPT: &str = include_str!("system_prompt.md");

/// Model-edit consumer backed by the shared configured provider client.
pub struct ModelEditBackend {
    client: OpenAiCompatibleClient,
}

impl std::fmt::Debug for ModelEditBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelEditBackend")
            .field("client", &self.client)
            .finish()
    }
}

impl ModelEditBackend {
    pub(crate) fn new(client: OpenAiCompatibleClient) -> Self {
        Self { client }
    }

    fn build_prompt(request: &AiRequest) -> String {
        let mut prompt = String::new();

        if request.is_retry {
            prompt.push_str("The previous code failed with the following error. ");
            prompt.push_str("Please fix it:\n\n");
            if let Some(traceback) = &request.traceback {
                prompt.push_str("```\n");
                prompt.push_str(traceback);
                prompt.push_str("\n```\n\n");
            }
        }

        if let Some(context) = &request.geometry_context {
            prompt.push_str(&format!(
                "The user selected a geometry element in the viewport: {}.\n",
                context.element.display_label()
            ));
            match &context.provenance {
                LedgerValue::Resolved(entry) => {
                    prompt.push_str(&format!(
                        "It was {} {} at line {}: `{}`\n",
                        entry.relation.display_phrase(),
                        entry.operation.display_name(),
                        entry.source.line,
                        entry.source.code
                    ));
                }
                LedgerValue::Ambiguous(candidates) => {
                    prompt.push_str(
                        "Its source is ambiguous; it was produced by one of these lines \
                         (decide from the measurements and the user's words, and say \
                         which you chose in the summary):\n",
                    );
                    for candidate in candidates {
                        prompt.push_str(&format!(
                            "- {} {} at line {}: `{}`\n",
                            candidate.relation.display_phrase(),
                            candidate.operation.display_name(),
                            candidate.source.line,
                            candidate.source.code
                        ));
                    }
                }
                LedgerValue::Untraced => {
                    prompt.push_str(
                        "No source line is known for it (it came from an operation \
                         CADmark cannot trace). Locate it from the measurements below.\n",
                    );
                }
            }

            if !context.identification.is_empty() {
                prompt.push_str("Measured geometry of the selected element:\n");
                let mut identification: Vec<_> = context.identification.iter().collect();
                identification.sort();
                for (key, value) in identification {
                    prompt.push_str(&format!("- {key}: {value}\n"));
                }
            }
            prompt.push('\n');
        }

        if let Some(documentation) = &request.doc_context {
            prompt.push_str("Relevant build123d API reference:\n\n");
            prompt.push_str(documentation);
            prompt.push_str("\n\n");
        }

        prompt.push_str("User message: ");
        prompt.push_str(&request.user_message);
        prompt.push_str("\n\nCurrent build123d code:\n```python\n");
        prompt.push_str(&request.current_code);
        prompt.push_str("\n```\n\n");
        prompt.push_str(
            "Respond with the complete modified build123d code in a ```python code block, \
             followed by a one-line summary of the change prefixed with 'Summary: '. \
             If you had any difficulty understanding the request, lacked context, or \
             are uncertain about part of the code, add a line prefixed with 'Notes: ' \
             explaining the issue. Write nothing else outside the code block, the \
             summary line, and the optional notes line.",
        );

        prompt
    }

    /// The chat message is the summary plus any notes: the code itself is
    /// visible in the viewport and the script, so repeating it in chat only
    /// buries the sentence the user needs.
    fn parse_response(raw: &str) -> Result<AiResponse, BackendError> {
        let code = extract_code_block(raw, "python")
            .ok_or_else(|| BackendError::ParseError("no Python code block in response".into()))?;
        let field = |name: &str| {
            raw.lines()
                .map(str::trim)
                .find_map(|line| line.strip_prefix(name))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let summary = field("Summary:").unwrap_or_else(|| "AI modification".to_string());
        let notes = field("Notes:");
        let message = match notes {
            Some(notes) => format!("{summary}\n\nNotes: {notes}"),
            None => summary.clone(),
        };

        Ok(AiResponse {
            code,
            summary,
            message,
        })
    }

    pub(crate) async fn smoke_test_exact_sentinel(
        &self,
        sentinel: &str,
    ) -> Result<(), BackendError> {
        let instructions = "Return exactly the requested sentinel and no other text.";
        let input = format!("Return exactly: {sentinel}");
        let output = self.client.request_text(instructions, &input).await?;
        if output.trim() == sentinel {
            Ok(())
        } else {
            Err(BackendError::ParseError(
                "live smoke response did not match the requested sentinel".to_string(),
            ))
        }
    }
}

impl AiBackend for ModelEditBackend {
    fn request(
        &self,
        request: AiRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse, BackendError>> + Send + '_>> {
        Box::pin(async move {
            let prompt = Self::build_prompt(&request);
            let raw = self.client.request_text(SYSTEM_PROMPT, &prompt).await?;
            Self::parse_response(&raw)
        })
    }
}

fn extract_code_block(text: &str, language: &str) -> Option<String> {
    let fence_start = format!("```{language}");
    let start = text.find(&fence_start)?;
    let content_start = start + fence_start.len();
    let content_start = text[content_start..].find('\n')? + content_start + 1;
    let end = text[content_start..].find("```")? + content_start;
    Some(text[content_start..end].trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_python_code_block() {
        let raw =
            "Text\n```python\nfrom build123d import *\nbox = Box(10, 10, 10)\n```\nSummary: Box";
        assert_eq!(
            extract_code_block(raw, "python").unwrap(),
            "from build123d import *\nbox = Box(10, 10, 10)"
        );
    }

    #[test]
    fn prompt_preserves_chat_retry_geometry_and_documentation_context() {
        use cadmark_core::geometry::{FaceId, GeometryContext, TopologyElement};
        use cadmark_core::ledger::{
            LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
        };

        let mut identification = std::collections::HashMap::new();
        identification.insert("surface".to_string(), "plane".to_string());
        let request = AiRequest::from_spatial_comment(
            "box = Box(1, 1, 1)".to_string(),
            "round this".to_string(),
            GeometryContext {
                element: TopologyElement::Face(FaceId(3)),
                provenance: LedgerValue::Resolved(ProvenanceEntry {
                    source: SourceRef {
                        line: 5,
                        code: "box = Box(1, 1, 1)".to_string(),
                    },
                    operation: SemanticOperation::Box,
                    operation_id: 1,
                    relation: ProvenanceRelation::Generated,
                }),
                identification,
            },
        )
        .with_doc_context("fillet(objects, radius)".to_string())
        .retry_with_traceback("distinctive traceback".to_string());

        let prompt = ModelEditBackend::build_prompt(&request);
        for expected in [
            "round this",
            "box = Box(1, 1, 1)",
            "face 3",
            "created by box at line 5",
            "surface: plane",
            "fillet(objects, radius)",
            "distinctive traceback",
        ] {
            assert!(prompt.contains(expected), "missing `{expected}`");
        }
    }

    #[test]
    fn prompt_states_ambiguous_and_untraced_sources_honestly() {
        use cadmark_core::geometry::{EdgeId, FaceId, GeometryContext, TopologyElement};
        use cadmark_core::ledger::{
            LedgerValue, ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
        };

        let entry = |line: u32, operation: SemanticOperation| ProvenanceEntry {
            source: SourceRef {
                line,
                code: format!("line {line}"),
            },
            operation,
            operation_id: u64::from(line),
            relation: ProvenanceRelation::Modified,
        };
        let ambiguous = AiRequest::from_spatial_comment(
            "code".into(),
            "comment".into(),
            GeometryContext {
                element: TopologyElement::Face(FaceId(1)),
                provenance: LedgerValue::Ambiguous(vec![
                    entry(2, SemanticOperation::Box),
                    entry(3, SemanticOperation::Fillet),
                ]),
                identification: Default::default(),
            },
        );
        let prompt = ModelEditBackend::build_prompt(&ambiguous);
        assert!(prompt.contains("ambiguous"));
        assert!(prompt.contains("modified by box at line 2"));
        assert!(prompt.contains("modified by fillet at line 3"));

        let untraced = AiRequest::from_spatial_comment(
            "code".into(),
            "comment".into(),
            GeometryContext {
                element: TopologyElement::Edge(EdgeId(4)),
                provenance: LedgerValue::Untraced,
                identification: Default::default(),
            },
        );
        let prompt = ModelEditBackend::build_prompt(&untraced);
        assert!(prompt.contains("edge 4"));
        assert!(prompt.contains("No source line is known"));
    }

    #[test]
    fn parses_response_code_summary_and_notes() {
        let raw = "```python\nresult = Box(2, 2, 2)\n```\nSummary: Resize\nNotes: Check fit";
        let response = ModelEditBackend::parse_response(raw).unwrap();
        assert_eq!(response.code, "result = Box(2, 2, 2)");
        assert_eq!(response.summary, "Resize");
        assert_eq!(response.message, "Resize\n\nNotes: Check fit");
    }

    #[test]
    fn chat_message_omits_the_code_and_repeats_nothing() {
        let raw = "Here you go:\n```python\nresult = Box(2, 2, 2)\n```\n  Summary: Made a box\n";
        let response = ModelEditBackend::parse_response(raw).unwrap();
        assert_eq!(response.message, "Made a box");
        assert!(!response.message.contains("```"));
    }

    #[test]
    fn rejects_response_without_python_block() {
        assert!(matches!(
            ModelEditBackend::parse_response("plain text"),
            Err(BackendError::ParseError(_))
        ));
    }
}
