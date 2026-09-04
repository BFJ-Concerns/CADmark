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
    use cadmark_core::ledger::SemanticOperation;

    #[derive(Debug, Clone, Copy)]
    enum DocumentationSource {
        Objects,
        Operations,
        DirectApi,
    }

    impl DocumentationSource {
        fn contents(self) -> &'static str {
            match self {
                Self::Objects => OBJECTS_REFERENCE,
                Self::Operations => OPERATIONS_REFERENCE,
                Self::DirectApi => DIRECT_API_REFERENCE,
            }
        }
    }

    /// Maps every operation whose provenance CADmark instruments to its
    /// explanatory build123d documentation entry. This match deliberately has
    /// no catch-all: an added `SemanticOperation` cannot compile until its
    /// documentation contract is recorded here.
    fn documentation_for(
        operation: SemanticOperation,
    ) -> (&'static str, DocumentationSource, &'static str) {
        match operation {
            SemanticOperation::Box => (
                "Box",
                DocumentationSource::Objects,
                "**Box** - Box defined by length, width, height",
            ),
            SemanticOperation::Cylinder => (
                "Cylinder",
                DocumentationSource::Objects,
                "**Cylinder** - Cylinder defined by radius and height",
            ),
            SemanticOperation::Sphere => (
                "Sphere",
                DocumentationSource::Objects,
                "**Sphere** - Sphere defined by radius and arc angles",
            ),
            SemanticOperation::Cone => (
                "Cone",
                DocumentationSource::Objects,
                "**Cone** - Cone defined by radii and height",
            ),
            SemanticOperation::Torus => (
                "Torus",
                DocumentationSource::Objects,
                "**Torus** - Torus defined by major and minor radii",
            ),
            SemanticOperation::Wedge => (
                "Wedge",
                DocumentationSource::Objects,
                "**Wedge** - Wedge defined by lengths along multiple axes",
            ),
            SemanticOperation::Extrude => (
                "extrude",
                DocumentationSource::Operations,
                "### extrude\nDraw a 2D Shape into a 3D solid",
            ),
            SemanticOperation::Revolve => (
                "revolve",
                DocumentationSource::Operations,
                "### revolve\nRotate a 2D shape around an axis",
            ),
            SemanticOperation::Loft => (
                "loft",
                DocumentationSource::Operations,
                "### loft\nCreate a 3D form by interpolating",
            ),
            SemanticOperation::Sweep => (
                "sweep",
                DocumentationSource::Operations,
                "### sweep\nExtrude a 2D or 3D section",
            ),
            SemanticOperation::Thicken => (
                "thicken",
                DocumentationSource::Operations,
                "### thicken\nExpand a 2D face into a 3D solid",
            ),
            SemanticOperation::Shell => (
                "offset",
                DocumentationSource::Operations,
                "### offset\nInset or outset a shape",
            ),
            SemanticOperation::Draft => (
                "draft",
                DocumentationSource::Operations,
                "### draft\nApply a taper angle",
            ),
            SemanticOperation::Split => (
                "split",
                DocumentationSource::Operations,
                "### split\nDivide an object by a plane",
            ),
            SemanticOperation::BooleanFuse => (
                "fuse",
                DocumentationSource::DirectApi,
                "fuse(**to_fuse: Shape*",
            ),
            SemanticOperation::BooleanCut => (
                "cut",
                DocumentationSource::DirectApi,
                "cut(**to_cut: Shape*)",
            ),
            SemanticOperation::BooleanCommon => (
                "common",
                DocumentationSource::DirectApi,
                "intersect(**to_intersect: Shape | Vector | Location | Axis | Plane*",
            ),
            SemanticOperation::Fillet => (
                "fillet",
                DocumentationSource::Operations,
                "### fillet\nRadius a vertex or edge",
            ),
            SemanticOperation::Chamfer => (
                "chamfer",
                DocumentationSource::Operations,
                "### chamfer\nBevel a vertex or edge",
            ),
        }
    }

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
        // word in a table or a neighbouring API name. The exhaustive mapping
        // above ties this list to the provenance instrumenter's operation
        // vocabulary: adding an operation requires a documentation anchor.
        for operation in SemanticOperation::ALL.iter().copied() {
            let (construct, source, anchor) = documentation_for(operation);
            assert!(
                source.contents().contains(anchor),
                "{source:?} reference is missing the documented {construct} entry"
            );
            assert!(
                DOC_CORPUS.contains(source.contents()),
                "lookup_docs payload no longer contains the {source:?} reference for {construct}"
            );
        }
        // Location patterns and mirror/rotate/scale transforms are real
        // consumer constructs but are not provenance operations, so they
        // remain separately explicit.
        for (construct, anchor) in [
            (
                "mirror",
                "### mirror\nMirror the shape about a specified plane",
            ),
            ("scale", "### scale\nChange the size of a shape"),
        ] {
            assert!(
                OPERATIONS_REFERENCE.contains(anchor),
                "operations reference is missing the documented {construct} entry"
            );
            assert!(
                DOC_CORPUS.contains(OPERATIONS_REFERENCE),
                "lookup_docs payload no longer contains the operations reference for {construct}"
            );
        }
        for (construct, anchor) in [
            (
                "Locations",
                "`Locations` - Use this to define a specific location",
            ),
            (
                "GridLocations",
                "`GridLocations` - Arrange objects in a grid pattern",
            ),
            (
                "PolarLocations",
                "`PolarLocations` - Position objects in a circular pattern",
            ),
            (
                "HexLocations",
                "`HexLocations` - Arrange objects in a hexagonal grid",
            ),
            (
                "rotate",
                "**Rotation:** Rotate a shape around a specified axis",
            ),
            ("rotate API", "shape.rotate(Axis, angle_in_degrees)"),
        ] {
            assert!(
                MOVING_OBJECTS_REFERENCE.contains(anchor),
                "moving-objects reference is missing the documented {construct} entry"
            );
        }
        assert!(
            DOC_CORPUS.contains(MOVING_OBJECTS_REFERENCE),
            "lookup_docs payload no longer contains the moving-objects reference"
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
