//! Provider-independent model-edit prompting and response parsing.

use std::future::Future;
use std::pin::Pin;

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
                "The user selected a geometry element ({})",
                describe_element(&context.element)
            ));
            prompt.push_str(&format!(
                " attributed to {:?} ({:?}) at line {}: `{}`",
                context.provenance.operation,
                context.provenance.relation,
                context.provenance.source.line,
                context.provenance.source.code
            ));
            prompt.push_str(".\n\n");

            if !context.identification.is_empty() {
                prompt.push_str("Additional context:\n");
                for (key, value) in &context.identification {
                    prompt.push_str(&format!("- {key}: {value}\n"));
                }
                prompt.push('\n');
            }
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
             explaining the issue.",
        );

        prompt
    }

    fn parse_response(raw: &str) -> Result<AiResponse, BackendError> {
        let code = extract_code_block(raw, "python")
            .ok_or_else(|| BackendError::ParseError("no Python code block in response".into()))?;
        let summary = raw
            .lines()
            .find(|line| line.starts_with("Summary: "))
            .map(|line| line.trim_start_matches("Summary: ").to_string())
            .unwrap_or_else(|| "AI modification".to_string());
        let notes = raw
            .lines()
            .find(|line| line.starts_with("Notes: "))
            .map(|line| line.trim_start_matches("Notes: ").to_string());
        let message = match notes {
            Some(notes) => format!("{raw}\n\n**AI notes:** {notes}"),
            None => raw.to_string(),
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

fn describe_element(element: &cadmark_core::geometry::TopologyElement) -> String {
    match element {
        cadmark_core::geometry::TopologyElement::Face(id) => format!("face #{}", id.0),
        cadmark_core::geometry::TopologyElement::Edge(id) => format!("edge #{}", id.0),
        cadmark_core::geometry::TopologyElement::Vertex(id) => format!("vertex #{}", id.0),
    }
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
            ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef,
        };

        let mut identification = std::collections::HashMap::new();
        identification.insert("position".to_string(), "top face".to_string());
        let request = AiRequest::from_spatial_comment(
            "box = Box(1, 1, 1)".to_string(),
            "round this".to_string(),
            GeometryContext {
                element: TopologyElement::Face(FaceId(3)),
                provenance: ProvenanceEntry {
                    source: SourceRef {
                        line: 5,
                        code: "box = Box(1, 1, 1)".to_string(),
                    },
                    operation: SemanticOperation::Box,
                    operation_id: 1,
                    relation: ProvenanceRelation::Generated,
                },
                identification,
            },
        )
        .with_doc_context("fillet(objects, radius)".to_string())
        .retry_with_traceback("distinctive traceback".to_string());

        let prompt = ModelEditBackend::build_prompt(&request);
        for expected in [
            "round this",
            "box = Box(1, 1, 1)",
            "face #3",
            "line 5",
            "position: top face",
            "fillet(objects, radius)",
            "distinctive traceback",
        ] {
            assert!(prompt.contains(expected), "missing `{expected}`");
        }
    }

    #[test]
    fn parses_response_code_summary_and_notes() {
        let raw = "```python\nresult = Box(2, 2, 2)\n```\nSummary: Resize\nNotes: Check fit";
        let response = ModelEditBackend::parse_response(raw).unwrap();
        assert_eq!(response.code, "result = Box(2, 2, 2)");
        assert_eq!(response.summary, "Resize");
        assert!(response.message.contains("**AI notes:** Check fit"));
    }

    #[test]
    fn rejects_response_without_python_block() {
        assert!(matches!(
            ModelEditBackend::parse_response("plain text"),
            Err(BackendError::ParseError(_))
        ));
    }
}
