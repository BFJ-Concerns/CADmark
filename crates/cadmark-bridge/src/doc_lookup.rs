// Documentation lookup pre-processing agent.
//
// Searches the build123d documentation corpus for API references
// relevant to the user's request. Runs before the design agent so
// it has exact signatures and usage patterns to work from, countering
// incorrect syntax from general training knowledge.
//
// Uses Haiku via `claude --print --model haiku` for cheap, fast lookup.
// Failure is non-fatal — the design agent proceeds without extra docs.

/// System prompt instructing Haiku to act as a reference librarian.
const LOOKUP_PROMPT: &str = include_str!("doc_lookup_prompt.md");

/// Full build123d documentation corpus, baked in at compile time.
/// Ordered from conceptual overview to detailed API reference so Haiku
/// encounters the high-level context before the exhaustive signatures.
const DOC_CORPUS: &str = concat!(
    // Conceptual foundations
    include_str!("../../../docs/build123d/introduction.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/key_concepts.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/key_concepts_builder.md"),
    "\n\n---\n\n",
    // Builder pattern and objects
    include_str!("../../../docs/build123d/builders.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/objects.md"),
    "\n\n---\n\n",
    // Operations and topology
    include_str!("../../../docs/build123d/operations.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/topology_selection.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/moving_objects.md"),
    "\n\n---\n\n",
    // Quick reference
    include_str!("../../../docs/build123d/cheat_sheet.md"),
    "\n\n---\n\n",
    // Builder API reference (common API for all builders)
    include_str!("../../../docs/build123d/builder_api_reference.md"),
    "\n\n---\n\n",
    // Full direct API reference — the exhaustive signature list
    include_str!("../../../docs/build123d/direct_api_reference.md"),
    "\n\n---\n\n",
    // Supplementary material
    include_str!("../../../docs/build123d/introductory_examples.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/tips.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/import_export.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/assemblies.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/joints.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/debugging_logging.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/advanced.md"),
    "\n\n---\n\n",
    // Tutorials
    include_str!("../../../docs/build123d/tutorials/design.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/tutorials/lego.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/tutorials/selectors.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/tutorials/joints.md"),
    "\n\n---\n\n",
    include_str!("../../../docs/build123d/tutorials/surface_modeling.md"),
);

/// Documentation lookup agent — searches build123d docs for API
/// references relevant to the user's request.
///
/// Stateless: the doc corpus and system prompt are compile-time
/// constants, so no initialisation or shared state is needed.
pub struct DocLookup;

impl DocLookup {
    /// Search the documentation corpus for content relevant to the
    /// user's request. Returns extracted API references and usage
    /// patterns, or `None` if the lookup fails for any reason.
    ///
    /// Failure is intentionally non-fatal — a warning is logged but
    /// the design agent proceeds without extra documentation context.
    pub async fn lookup(
        user_message: &str,
        message_history: &[String],
    ) -> Option<String> {
        let prompt = Self::build_prompt(user_message, message_history);

        let output = tokio::process::Command::new("claude")
            .arg("--print")
            .arg("--model")
            .arg("haiku")
            .arg("--output-format")
            .arg("text")
            .arg("--system-prompt")
            .arg(LOOKUP_PROMPT)
            .arg("--no-session-persistence")
            .arg("--bare")
            .arg(&prompt)
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if text.is_empty() || text == "No relevant documentation found." {
                    log::debug!("Doc lookup returned no relevant results");
                    None
                } else {
                    log::debug!(
                        "Doc lookup returned {} bytes of API reference",
                        text.len()
                    );
                    Some(text)
                }
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                log::warn!(
                    "Doc lookup agent exited with {}: {stderr}",
                    out.status
                );
                None
            }
            Err(e) => {
                log::warn!("Failed to invoke doc lookup agent: {e}");
                None
            }
        }
    }

    /// Assemble the prompt sent to Haiku. Contains the conversation
    /// history, the current request, and the full documentation corpus.
    fn build_prompt(user_message: &str, message_history: &[String]) -> String {
        let mut prompt = String::new();

        // Recent conversation history for context (e.g. knowing what
        // "make it taller" refers to).
        if !message_history.is_empty() {
            prompt.push_str("<conversation_history>\n");
            for msg in message_history {
                prompt.push_str("- ");
                prompt.push_str(msg);
                prompt.push('\n');
            }
            prompt.push_str("</conversation_history>\n\n");
        }

        prompt.push_str("<current_request>\n");
        prompt.push_str(user_message);
        prompt.push_str("\n</current_request>\n\n");

        prompt.push_str("<build123d_documentation>\n");
        prompt.push_str(DOC_CORPUS);
        prompt.push_str("\n</build123d_documentation>");

        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_includes_all_sections() {
        let prompt = DocLookup::build_prompt(
            "fillet the top edges",
            &["make a box".to_string(), "add a hole".to_string()],
        );
        // Conversation history present with both messages.
        assert!(prompt.contains("<conversation_history>"));
        assert!(prompt.contains("- make a box"));
        assert!(prompt.contains("- add a hole"));
        // Current request present.
        assert!(prompt.contains("<current_request>"));
        assert!(prompt.contains("fillet the top edges"));
        // Documentation corpus present.
        assert!(prompt.contains("<build123d_documentation>"));
        // Spot-check that the corpus actually contains doc content.
        assert!(prompt.contains("build123d"));
    }

    #[test]
    fn build_prompt_omits_history_when_empty() {
        let prompt = DocLookup::build_prompt("make a box", &[]);
        assert!(!prompt.contains("<conversation_history>"));
        assert!(prompt.contains("<current_request>"));
        assert!(prompt.contains("make a box"));
    }

    #[test]
    fn doc_corpus_is_non_empty() {
        // Sanity check that the compile-time corpus loaded successfully.
        assert!(
            DOC_CORPUS.len() > 100_000,
            "Doc corpus should be >100KB, got {} bytes",
            DOC_CORPUS.len()
        );
    }

    #[test]
    fn doc_corpus_contains_key_api_entries() {
        // The direct API reference should include these fundamental types.
        assert!(DOC_CORPUS.contains("class"));
        assert!(DOC_CORPUS.contains("Box"));
        assert!(DOC_CORPUS.contains("Cylinder"));
        assert!(DOC_CORPUS.contains("fillet"));
        assert!(DOC_CORPUS.contains("chamfer"));
        assert!(DOC_CORPUS.contains("extrude"));
    }
}
