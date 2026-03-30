// Claude Code subprocess implementation of the AI backend.
//
// Invokes `claude` in headless mode, passing structured context
// and parsing the response for modified code and summary.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::backend::{AiBackend, AiResponse, BackendError};
use crate::context::AiRequest;

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
            if let Some(line) = ctx.source_line {
                prompt.push_str(&format!(" generated at line {line}"));
            }
            if let Some(code) = &ctx.source_code {
                prompt.push_str(&format!(": `{code}`"));
            }
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

        prompt.push_str("User message: ");
        prompt.push_str(&request.user_message);
        prompt.push_str("\n\nCurrent build123d code:\n```python\n");
        prompt.push_str(&request.current_code);
        prompt.push_str("\n```\n\n");
        prompt.push_str(
            "Respond with the complete modified build123d code in a ```python code block, \
             followed by a one-line summary of the change prefixed with 'Summary: '.",
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

        Ok(AiResponse {
            code,
            summary,
            message: raw.to_string(),
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

            // Invoke Claude Code in headless/print mode.
            let output = tokio::process::Command::new("claude")
                .arg("--print")
                .arg("--output-format")
                .arg("text")
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
        let request = AiRequest::from_chat(
            "bad code".to_string(),
            "fix it".to_string(),
        ).retry_with_traceback("NameError: name 'x' is not defined".to_string());
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
            source_line: Some(5),
            source_code: Some("box = Box(10, 10, 10)".to_string()),
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
            source_line: None,
            source_code: None,
            identification,
        };
        let request = AiRequest::from_spatial_comment(
            "code".to_string(),
            "fillet this".to_string(),
            context,
        );
        let prompt = backend.build_prompt(&request);
        assert!(prompt.contains("position: top face"));
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
