//! Documentation lookup consumer.
//
// Searches the build123d documentation corpus for API references
// relevant to the user's request. Runs before the design agent so
// it has exact signatures and usage patterns to work from, countering
// incorrect syntax from general training knowledge.
//
use crate::openai_compatible::OpenAiCompatibleClient;

/// System prompt for the reference lookup consumer.
const LOOKUP_PROMPT: &str = include_str!("doc_lookup_prompt.md");

/// Full build123d documentation corpus, baked in at compile time.
/// Ordered from conceptual overview to detailed API reference so the lookup
/// consumer encounters high-level context before exhaustive signatures.
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

/// Documentation lookup consumer sharing the configured provider client.
pub struct DocLookup {
    client: OpenAiCompatibleClient,
}

impl std::fmt::Debug for DocLookup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DocLookup")
            .field("client", &self.client)
            .finish()
    }
}

impl DocLookup {
    pub(crate) fn new(client: OpenAiCompatibleClient) -> Self {
        Self { client }
    }

    /// Search the documentation corpus for content relevant to the
    /// user's request. Returns extracted API references and usage
    /// patterns, or `None` if the lookup fails for any reason.
    ///
    /// Failure is intentionally non-fatal — a warning is logged but
    /// the design agent proceeds without extra documentation context.
    pub async fn lookup(&self, user_message: &str, message_history: &[String]) -> Option<String> {
        let prompt = Self::build_prompt(user_message, message_history);
        match self.client.request_text(LOOKUP_PROMPT, &prompt).await {
            Ok(text)
                if text.trim().is_empty() || text.trim() == "No relevant documentation found." =>
            {
                log::debug!("Doc lookup returned no relevant results");
                None
            }
            Ok(text) => {
                log::debug!("Doc lookup returned {} bytes of API reference", text.len());
                Some(text)
            }
            Err(error) => {
                log::warn!("Documentation lookup failed: {error}");
                None
            }
        }
    }

    /// Assemble the lookup prompt. Contains the conversation
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
