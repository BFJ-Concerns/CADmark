//! Documentation lookup: the `lookup_docs` tool's implementation.
//!
//! Answers the model's build123d API question from the documentation
//! corpus bundled into the binary, so the modelling agent works from exact
//! signatures rather than training-data recall.

use cadmark_core::cancellation::CancelFlag;

use crate::openai_compatible::OpenAiCompatibleClient;

/// System prompt for the reference lookup consumer.
const LOOKUP_PROMPT: &str = include_str!("doc_lookup_prompt.md");

/// The exact sentence the lookup prompt asks for when nothing applies.
pub const NO_RESULT: &str = "No relevant documentation found.";

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

/// The operations reference is a distinct part of the payload because it
/// defines the builder-operation vocabulary CADmark instruments.
#[cfg(test)]
const OPERATIONS_REFERENCE: &str = include_str!("../../../docs/build123d/operations.md");
#[cfg(test)]
const OBJECTS_REFERENCE: &str = include_str!("../../../docs/build123d/objects.md");
#[cfg(test)]
const MOVING_OBJECTS_REFERENCE: &str = include_str!("../../../docs/build123d/moving_objects.md");
/// Direct API reference supplies the boolean-operation signature not present
/// in the builder operations table.
#[cfg(test)]
const DIRECT_API_REFERENCE: &str = include_str!("../../../docs/build123d/direct_api_reference.md");

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

    /// Answer a documentation question. The result is what the model
    /// reads as the tool's output: extracted API references, the corpus's
    /// own "nothing relevant" sentence, or the reason the lookup failed —
    /// a failed lookup is information the model acts on, not a silent gap.
    pub async fn lookup(&self, query: &str, cancel: CancelFlag) -> String {
        let prompt = Self::build_prompt(query);
        match self
            .client
            .request_text(LOOKUP_PROMPT, &prompt, cancel)
            .await
        {
            Ok(text) if text.trim().is_empty() => NO_RESULT.to_string(),
            Ok(text) => {
                log::debug!("Doc lookup returned {} bytes of API reference", text.len());
                text
            }
            Err(error) => {
                log::warn!("Documentation lookup failed: {error}");
                format!("Documentation lookup failed ({error}); rely on what you know.")
            }
        }
    }

    /// Assemble the lookup prompt: the question and the full corpus.
    fn build_prompt(query: &str) -> String {
        let mut prompt = String::new();
        prompt.push_str("<current_request>\n");
        prompt.push_str(query);
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
    fn build_prompt_carries_the_query_and_the_corpus() {
        let prompt = DocLookup::build_prompt("fillet the top edges");
        assert!(prompt.contains("<current_request>"));
        assert!(prompt.contains("fillet the top edges"));
        assert!(prompt.contains("<build123d_documentation>"));
        assert!(prompt.contains("build123d"));
    }

    #[tokio::test]
    async fn a_failed_lookup_tells_the_model_so() {
        use crate::openai_compatible::recording::{provider_failure, recording_server};
        let (base_url, _, server) = recording_server(vec![provider_failure(
            500,
            "server_error",
            "boom",
            "exploded",
        )])
        .await;
        let client = crate::config::AiConfiguration {
            base_url,
            model: "m".into(),
            accepts_images: false,
            allow_insecure_http: true,
        }
        .build_client(None)
        .unwrap();
        let answer = DocLookup::new(client)
            .lookup("fillet", CancelFlag::new())
            .await;
        server.await.unwrap();
        assert!(answer.starts_with("Documentation lookup failed"));
        assert!(answer.contains("boom"));
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

    #[test]
    fn documentation_consumer_covers_instrumented_build123d_operations() {
        // Each anchor is the construct's explanatory entry, not merely a
        // word in a table or a neighbouring API name. The whole source must
        // also reach lookup_docs, so removing either the entry or its source
        // from the payload reddens this test.
        for (construct, anchor) in [
            ("Box", "**Box** - Box defined by length, width, height"),
            (
                "Cylinder",
                "**Cylinder** - Cylinder defined by radius and height",
            ),
            (
                "Sphere",
                "**Sphere** - Sphere defined by radius and arc angles",
            ),
            ("Cone", "**Cone** - Cone defined by radii and height"),
            (
                "Torus",
                "**Torus** - Torus defined by major and minor radii",
            ),
            (
                "Wedge",
                "**Wedge** - Wedge defined by lengths along multiple axes",
            ),
        ] {
            assert!(
                OBJECTS_REFERENCE.contains(anchor),
                "objects reference is missing the documented {construct} entry"
            );
        }
        for (construct, anchor) in [
            ("extrude", "### extrude\nDraw a 2D Shape into a 3D solid"),
            ("revolve", "### revolve\nRotate a 2D shape around an axis"),
            ("loft", "### loft\nCreate a 3D form by interpolating"),
            ("sweep", "### sweep\nExtrude a 2D or 3D section"),
            ("thicken", "### thicken\nExpand a 2D face into a 3D solid"),
            ("draft", "### draft\nApply a taper angle"),
            ("split", "### split\nDivide an object by a plane"),
            ("fillet", "### fillet\nRadius a vertex or edge"),
            ("chamfer", "### chamfer\nBevel a vertex or edge"),
            (
                "mirror",
                "### mirror\nMirror the shape about a specified plane",
            ),
            ("scale", "### scale\nChange the size of a shape"),
            ("offset", "### offset\nInset or outset a shape"),
        ] {
            assert!(
                OPERATIONS_REFERENCE.contains(anchor),
                "operations reference is missing the documented {construct} entry"
            );
        }
        assert!(
            OBJECTS_REFERENCE.contains("**Box**") && DOC_CORPUS.contains(OBJECTS_REFERENCE),
            "lookup_docs payload no longer contains the objects reference"
        );
        assert!(
            DOC_CORPUS.contains(OPERATIONS_REFERENCE),
            "lookup_docs payload no longer contains the operations reference"
        );
        assert!(
            MOVING_OBJECTS_REFERENCE
                .contains("`Locations` - Use this to define a specific location")
                && MOVING_OBJECTS_REFERENCE
                    .contains("`GridLocations` - Arrange objects in a grid pattern")
                && MOVING_OBJECTS_REFERENCE
                    .contains("`PolarLocations` - Position objects in a circular pattern")
                && MOVING_OBJECTS_REFERENCE
                    .contains("`HexLocations` - Arrange objects in a hexagonal grid")
                && MOVING_OBJECTS_REFERENCE
                    .contains("**Rotation:** Rotate a shape around a specified axis")
                && MOVING_OBJECTS_REFERENCE.contains("shape.rotate(Axis, angle_in_degrees)")
                && DOC_CORPUS.contains(MOVING_OBJECTS_REFERENCE),
            "lookup_docs payload is missing documented location patterns or rotate"
        );
        for (construct, anchor) in [
            ("Shell", "*class *Shell("),
            ("fuse", "fuse(**to_fuse: Shape*"),
            ("cut", "cut(**to_cut: Shape*)"),
            (
                "common",
                "intersect(**to_intersect: Shape | Vector | Location | Axis | Plane*",
            ),
        ] {
            assert!(
                DIRECT_API_REFERENCE.contains(anchor),
                "direct API reference is missing the documented {construct} entry"
            );
        }
        assert!(
            DOC_CORPUS.contains(DIRECT_API_REFERENCE),
            "lookup_docs payload no longer contains the direct API reference"
        );
    }
}
