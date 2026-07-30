// Claude Code subprocess implementation of the AI backend.
//
// Invokes `claude` in headless mode, passing structured context
// and parsing the response for modified code and summary.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::backend::{AiBackend, AiResponse, BackendError};
use crate::context::AiRequest;

/// System prompt giving the headless Claude build123d expertise and
/// parametric modelling instructions. Baked in at compile time.
const SYSTEM_PROMPT: &str = include_str!("system_prompt.md");

/// Claude Code AI backend — invokes the CLI as a subprocess.
pub struct ClaudeCodeBackend {
    /// Working directory for the Claude Code subprocess.
    /// Should be the project directory containing the .py file.
    project_dir: PathBuf,
}

impl ClaudeCodeBackend {
    pub fn new(project_dir: PathBuf) -> Self {
        Self { project_dir }
    }

    /// Build the prompt from the AI request.
    fn build_prompt(&self, request: &AiRequest) -> String {
        let mut prompt = String::new();

        if request.is_retry {
            prompt.push_str("The previous code failed with the following error. ");
            prompt.push_str("Please fix it:\n\n");
            if let Some(tb) = &request.traceback {
                prompt.push_str("```\n");
                prompt.push_str(tb);
                prompt.push_str("\n```\n\n");
            }
        }

        if let Some(ctx) = &request.geometry_context {
            prompt.push_str(&format!(
                "The user selected a geometry element ({})",
                describe_element(&ctx.element)
            ));
            prompt.push_str(&format!(
                " attributed to {:?} ({:?}) at line {}: `{}`",
                ctx.provenance.operation,
                ctx.provenance.relation,
                ctx.provenance.source.line,
                ctx.provenance.source.code
            ));
            prompt.push_str(".\n\n");

            // Include experimental identification data.
            if !ctx.identification.is_empty() {
                prompt.push_str("Additional context:\n");
                for (key, value) in &ctx.identification {
                    prompt.push_str(&format!("- {key}: {value}\n"));
                }
                prompt.push('\n');
            }
        }

        // Inject documentation references from the doc lookup agent
        // so the design agent has exact API signatures to work from.
        if let Some(doc_ctx) = &request.doc_context {
            prompt.push_str("Relevant build123d API reference:\n\n");
            prompt.push_str(doc_ctx);
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

    /// Parse Claude Code's response to extract code and summary.
    fn parse_response(&self, raw: &str) -> Result<AiResponse, BackendError> {
        // Extract the Python code block.
        let code = extract_code_block(raw, "python")
            .ok_or_else(|| BackendError::ParseError("no Python code block in response".into()))?;

        // Extract the summary line.
        let summary = raw
            .lines()
            .find(|l| l.starts_with("Summary: "))
            .map(|l| l.trim_start_matches("Summary: ").to_string())
            .unwrap_or_else(|| "AI modification".to_string());

        // Extract optional notes about issues the AI encountered.
        let notes = raw
            .lines()
            .find(|l| l.starts_with("Notes: "))
            .map(|l| l.trim_start_matches("Notes: ").to_string());

        // Build the chat message — include notes when present so the
        // user can see any concerns the AI flagged.
        let message = match &notes {
            Some(n) => format!("{raw}\n\n**AI notes:** {n}"),
            None => raw.to_string(),
        };

        Ok(AiResponse {
            code,
            summary,
            message,
        })
    }
}

impl AiBackend for ClaudeCodeBackend {
    fn request(
        &self,
        request: AiRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse, BackendError>> + Send + '_>> {
        Box::pin(async move {
            let prompt = self.build_prompt(&request);

            // Invoke Claude Code in headless/print mode with build123d
            // expertise via the system prompt.
            let output = tokio::process::Command::new("claude")
                .arg("--print")
                .arg("--output-format")
                .arg("text")
                .arg("--system-prompt")
                .arg(SYSTEM_PROMPT)
                .arg(&prompt)
                .current_dir(&self.project_dir)
                .output()
                .await
                .map_err(|e| BackendError::Unavailable(format!("failed to invoke claude: {e}")))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(BackendError::RequestFailed(format!(
                    "claude exited with {}: {stderr}",
                    output.status
                )));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            self.parse_response(&stdout)
        })
    }

    fn is_available(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async {
            tokio::process::Command::new("claude")
                .arg("--version")
                .output()
                .await
                .is_ok_and(|o| o.status.success())
        })
    }
}

/// Extract the contents of a fenced code block with the given language tag.
fn extract_code_block(text: &str, language: &str) -> Option<String> {
    let fence_start = format!("```{language}");
    let start = text.find(&fence_start)?;
    let content_start = start + fence_start.len();
    // Skip to the next newline after the opening fence.
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
    use cadmark_core::ledger::{ProvenanceEntry, ProvenanceRelation, SemanticOperation, SourceRef};

    fn provenance(line: u32, code: &str) -> ProvenanceEntry {
        ProvenanceEntry {
            source: SourceRef {
                line,
                code: code.to_string(),
            },
            operation: SemanticOperation::Box,
            operation_id: 1,
            relation: ProvenanceRelation::Generated,
        }
    }

    #[test]
    fn extract_python_code_block() {
        let text = "Here's the code:\n```python\nfrom build123d import *\nbox = Box(10, 10, 10)\n```\nSummary: Created a box";
        let code = extract_code_block(text, "python").unwrap();
        assert_eq!(code, "from build123d import *\nbox = Box(10, 10, 10)");
    }

    #[test]
    fn extract_no_code_block_returns_none() {
        let text = "No code here, just text.";
        assert!(extract_code_block(text, "python").is_none());
    }

    #[test]
    fn build_prompt_chat_message() {
        let backend = ClaudeCodeBackend::new(PathBuf::from("/tmp"));
        let request = AiRequest::from_chat(
            "box = Box(10, 10, 10)".to_string(),
            "make the box bigger".to_string(),
        );
        let prompt = backend.build_prompt(&request);
        assert!(prompt.contains("make the box bigger"));
        assert!(prompt.contains("box = Box(10, 10, 10)"));
        assert!(!prompt.contains("error"));
    }

    #[test]
    fn build_prompt_retry_includes_traceback() {
        let backend = ClaudeCodeBackend::new(PathBuf::from("/tmp"));
        let request = AiRequest::from_chat("bad code".to_string(), "fix it".to_string())
            .retry_with_traceback("NameError: name 'x' is not defined".to_string());
        let prompt = backend.build_prompt(&request);
        assert!(prompt.contains("previous code failed"));
        assert!(prompt.contains("NameError"));
    }

    #[test]
    fn build_prompt_spatial_comment_includes_geometry() {
        use cadmark_core::geometry::*;
        let backend = ClaudeCodeBackend::new(PathBuf::from("/tmp"));
        let context = GeometryContext {
            element: TopologyElement::Face(FaceId(3)),
            provenance: provenance(5, "box = Box(10, 10, 10)"),
            identification: std::collections::HashMap::new(),
        };
        let request = AiRequest::from_spatial_comment(
            "code".to_string(),
            "round this edge".to_string(),
            context,
        );
        let prompt = backend.build_prompt(&request);
        assert!(prompt.contains("face #3"));
        assert!(prompt.contains("line 5"));
        assert!(prompt.contains("box = Box(10, 10, 10)"));
    }

    #[test]
    fn build_prompt_with_identification_data() {
        use cadmark_core::geometry::*;
        let backend = ClaudeCodeBackend::new(PathBuf::from("/tmp"));
        let mut identification = std::collections::HashMap::new();
        identification.insert("position".to_string(), "top face".to_string());
        let context = GeometryContext {
            element: TopologyElement::Face(FaceId(0)),
            provenance: provenance(1, "box = Box(10, 10, 10)"),
            identification,
        };
        let request =
            AiRequest::from_spatial_comment("code".to_string(), "fillet this".to_string(), context);
        let prompt = backend.build_prompt(&request);
        assert!(prompt.contains("position: top face"));
    }

    #[test]
    fn build_prompt_with_doc_context() {
        let backend = ClaudeCodeBackend::new(PathBuf::from("/tmp"));
        let request = AiRequest::from_chat(
            "box = Box(10, 10, 10)".to_string(),
            "fillet the top edges".to_string(),
        )
        .with_doc_context("fillet(objects, radius)\n\nRound edges.".to_string());
        let prompt = backend.build_prompt(&request);
        // Doc context appears before the user message.
        let doc_pos = prompt.find("Relevant build123d API reference").unwrap();
        let msg_pos = prompt.find("User message:").unwrap();
        assert!(doc_pos < msg_pos, "doc context should precede user message");
        assert!(prompt.contains("fillet(objects, radius)"));
    }

    #[test]
    fn parse_response_extracts_code_and_summary() {
        let backend = ClaudeCodeBackend::new(PathBuf::from("/tmp"));
        let raw = "I've updated the code:\n```python\nbox = Box(20, 20, 20)\n```\nSummary: Doubled box dimensions";
        let response = backend.parse_response(raw).unwrap();
        assert_eq!(response.code, "box = Box(20, 20, 20)");
        assert_eq!(response.summary, "Doubled box dimensions");
    }
}
