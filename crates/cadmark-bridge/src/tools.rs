// The tools CADmark offers the model during a turn, and the typed
// arguments each takes. The specs are the model's contract; the
// implementations live with the mechanisms they need (the kernel worker,
// the renderer) in the application's turn module.
//
// Adding a tool: a `ToolSpec` constructor here with its argument struct,
// an arm in the application's tool dispatch, and a paragraph in
// `system_prompt.md` telling the model when to use it.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::backend::ToolSpec;

/// The tool that runs a script: the model writes the complete build123d
/// file, CADmark executes it and returns the outcome.
pub const RUN_SCRIPT: &str = "run_script";
/// The tool that answers a build123d API question from the documentation.
pub const LOOKUP_DOCS: &str = "lookup_docs";
/// The tool that shows the model the current viewport.
pub const RENDER_VIEW: &str = "render_view";
/// The tool that lists the project's reference library, or reads one of
/// its images.
pub const REFERENCE_IMAGES: &str = "reference_images";
/// The tool that keeps an image in the reference library, or updates the
/// description of one already there.
pub const KEEP_REFERENCE: &str = "keep_reference";

/// Arguments of `run_script`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunScriptArgs {
    /// The complete script, replacing the file on disk.
    pub code: String,
    /// One line saying what changed, for the design step's record.
    pub summary: String,
}

/// Arguments of `lookup_docs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookupDocsArgs {
    /// What the model wants to know, in its own words.
    pub query: String,
}

/// Where the render tool looks from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderView {
    /// The user's current camera.
    Current,
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Isometric,
}

/// Arguments of `render_view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderViewArgs {
    pub view: RenderView,
}

/// Arguments of `reference_images`: no file lists the library; a file
/// name reads that image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceImagesArgs {
    #[serde(default)]
    pub file: Option<String>,
}

/// Where the image `keep_reference` keeps comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeepReferenceSource {
    /// An image attached to a message in this conversation, by the name
    /// the message shows for it.
    Attachment { name: String },
    /// A file already in the library: only its description changes.
    Library { file: String },
}

/// Arguments of `keep_reference`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeepReferenceArgs {
    pub source: KeepReferenceSource,
    /// The file name to keep it under; ignored for a library file.
    #[serde(default)]
    pub file: Option<String>,
    /// What the picture shows and what it is for, for the index.
    pub description: String,
}

pub fn run_script_spec() -> ToolSpec {
    ToolSpec {
        name: RUN_SCRIPT.to_string(),
        description: "Write the complete build123d script and execute it. Returns the model's \
                      measurements and validity, or the error to fix. The viewport shows the \
                      result immediately. Call again after fixing an error."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "The whole script file."},
                "summary": {"type": "string", "description": "One line saying what this version changes."}
            },
            "required": ["code", "summary"],
            "additionalProperties": false
        }),
    }
}

pub fn lookup_docs_spec() -> ToolSpec {
    ToolSpec {
        name: LOOKUP_DOCS.to_string(),
        description: "Look up build123d API signatures, worked modelling recipes, and FFF/FDM \
                      design guidance. Use for uncertain APIs or fiddly modelling tasks such as \
                      threaded bolts with a helix, fits, holes, shells, and fillets."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "What to look up, e.g. 'fillet edge selection by axis'."}
            },
            "required": ["query"],
            "additionalProperties": false
        }),
    }
}

pub fn render_view_spec() -> ToolSpec {
    ToolSpec {
        name: RENDER_VIEW.to_string(),
        description: "See the current model as a shaded image with edges, framed to the model, \
                      from a standard view or the user's current camera. Use to check what you \
                      made; it never replaces the geometry the user pointed at."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "view": {
                    "type": "string",
                    "enum": ["current", "front", "back", "left", "right", "top", "bottom", "isometric"]
                }
            },
            "required": ["view"],
            "additionalProperties": false
        }),
    }
}

pub fn reference_images_spec() -> ToolSpec {
    ToolSpec {
        name: REFERENCE_IMAGES.to_string(),
        description: "The project's reference library: photos and drawings kept in the \
                      project's references/ folder for every conversation. With no file, \
                      lists each image with its description from the index. With a file name, \
                      returns that image for you to look at."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "file": {"type": "string", "description": "A file name from the listing, to read that image. Omit to list the library."}
            },
            "additionalProperties": false
        }),
    }
}

pub fn keep_reference_spec() -> ToolSpec {
    ToolSpec {
        name: KEEP_REFERENCE.to_string(),
        description: "Keep an image attached to this conversation in the project's reference \
                      library so later conversations can find it, or update the description of \
                      an image already there. The description goes in the library's index: say \
                      what the picture shows and what it is used for."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "object",
                    "description": "Where the image is: {\"attachment\": {\"name\": ...}} for an image attached to a message in this conversation, by the name shown for it; {\"library\": {\"file\": ...}} to re-describe a file already in the library.",
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {"attachment": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"], "additionalProperties": false}},
                            "required": ["attachment"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {"library": {"type": "object", "properties": {"file": {"type": "string"}}, "required": ["file"], "additionalProperties": false}},
                            "required": ["library"],
                            "additionalProperties": false
                        }
                    ]
                },
                "file": {"type": "string", "description": "The file name to keep an attachment under, e.g. 'flange-top-view'. The extension follows the image format."},
                "description": {"type": "string", "description": "One or two sentences: what the picture shows and what it is for."}
            },
            "required": ["source", "description"],
            "additionalProperties": false
        }),
    }
}

/// The tools offered for one turn. The image tools — the render, and the
/// reference library — are offered only to a model that reads images; a
/// text-only model is told so in the instructions rather than handed
/// tools it cannot use.
pub fn tools_for(accepts_images: bool) -> Vec<ToolSpec> {
    let mut tools = vec![run_script_spec(), lookup_docs_spec()];
    if accepts_images {
        tools.push(render_view_spec());
        tools.push(reference_images_spec());
        tools.push(keep_reference_spec());
    }
    tools
}

/// What a model is told about tools it is not being offered. A model
/// that cannot read images would otherwise have to infer the image tools'
/// absence from a list it never sees in full, so the instructions say it
/// plainly instead.
pub fn unavailable_tools_note(accepts_images: bool) -> Option<&'static str> {
    (!accepts_images).then_some(
        "The `render_view`, `reference_images` and `keep_reference` tools are unavailable in \
         this session: the model in use cannot read images, and images attached to messages \
         are not sent. Judge the result from the measurements and validity the run reports, \
         and say so when you cannot check it by eye.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_that_cannot_see_is_told_the_render_tool_is_unavailable() {
        let note = unavailable_tools_note(false).expect("a text-only model is told");
        assert!(note.contains(RENDER_VIEW));
        assert!(note.contains("unavailable"));
        assert!(unavailable_tools_note(true).is_none());
    }

    #[test]
    fn tool_arguments_parse_from_the_models_json() {
        let run: RunScriptArgs =
            serde_json::from_value(json!({"code": "x = 1", "summary": "Set x"})).unwrap();
        assert_eq!(run.code, "x = 1");
        let render: RenderViewArgs = serde_json::from_value(json!({"view": "top"})).unwrap();
        assert_eq!(render.view, RenderView::Top);
        assert!(serde_json::from_value::<RenderViewArgs>(json!({"view": "sideways"})).is_err());
        let list: ReferenceImagesArgs = serde_json::from_value(json!({})).unwrap();
        assert_eq!(list.file, None);
        let read: ReferenceImagesArgs =
            serde_json::from_value(json!({"file": "flange.png"})).unwrap();
        assert_eq!(read.file.as_deref(), Some("flange.png"));
        let keep: KeepReferenceArgs = serde_json::from_value(json!({
            "source": {"attachment": {"name": "Flange"}},
            "file": "flange-top",
            "description": "Top view of the flange."
        }))
        .unwrap();
        assert_eq!(
            keep.source,
            KeepReferenceSource::Attachment {
                name: "Flange".into()
            }
        );
        let redescribe: KeepReferenceArgs = serde_json::from_value(json!({
            "source": {"library": {"file": "flange-top.png"}},
            "description": "Top view, bolt circle visible."
        }))
        .unwrap();
        assert_eq!(
            redescribe.source,
            KeepReferenceSource::Library {
                file: "flange-top.png".into()
            }
        );
        assert_eq!(redescribe.file, None);
    }

    #[test]
    fn the_image_tools_are_withheld_from_text_only_models() {
        let names = |accepts: bool| {
            tools_for(accepts)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(true),
            [
                RUN_SCRIPT,
                LOOKUP_DOCS,
                RENDER_VIEW,
                REFERENCE_IMAGES,
                KEEP_REFERENCE
            ]
        );
        assert_eq!(names(false), [RUN_SCRIPT, LOOKUP_DOCS]);
        let note = unavailable_tools_note(false).unwrap();
        assert!(note.contains(REFERENCE_IMAGES) && note.contains(KEEP_REFERENCE));
    }
}
