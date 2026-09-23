// The tools CADmark offers the model during a turn, and the typed
// arguments each takes. The specs are the model's contract; the
// implementations live with the mechanisms they need (the kernel worker,
// the renderer) in the application's turn module.
//
// Adding a tool: a `ToolSpec` constructor here with its argument struct,
// an arm in the application's tool dispatch, and a paragraph in
// `system_prompt.md` telling the model when to use it.
//
// The file tools follow the shape a CLI coding agent's do — replace the
// file, edit it by exact text, read it by line — so a change costs the
// model its lines rather than the whole script, and a scratch run
// answers a question without touching the file.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::backend::ToolSpec;

/// The tool that executes the script and rebuilds the model. Given the
/// complete file it replaces the script first; without it the file on
/// disk runs as `edit_script` left it.
pub const RUN_SCRIPT: &str = "run_script";
/// The tool that changes part of the script in place by exact text
/// replacement, the way a CLI agent's edit tool does. Nothing runs.
pub const EDIT_SCRIPT: &str = "edit_script";
/// The tool that shows the script, or a range of it, with line numbers.
pub const READ_SCRIPT: &str = "read_script";
/// The tool that runs scratch Python — after the current script, in its
/// namespace, or alone — for its printed output and last value, changing
/// nothing.
pub const RUN_PYTHON: &str = "run_python";
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
    /// The complete script, replacing the file on disk before the run;
    /// absent when the file is to run as it stands.
    #[serde(default)]
    pub code: Option<String>,
    /// One line saying what changed, for the design step's record.
    pub summary: String,
}

/// Arguments of `edit_script`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditScriptArgs {
    /// The exact text to replace, as it appears in the script.
    pub old_text: String,
    /// What replaces it.
    pub new_text: String,
    /// Replace every occurrence rather than requiring exactly one.
    #[serde(default)]
    pub replace_all: bool,
}

/// Arguments of `read_script`: a line range, or the whole file when both
/// ends are absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReadScriptArgs {
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub end_line: Option<u32>,
}

/// Arguments of `run_python`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPythonArgs {
    /// The Python to run.
    pub code: String,
    /// Run the snippet alone rather than after the current script.
    #[serde(default)]
    pub standalone: bool,
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
    /// An image attached to a message in this conversation, by the file
    /// name the message shows beside it. Two attachments may share the
    /// name the user knows them by; the file is unique.
    Attachment { file: String },
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
        description: "Execute the part script and rebuild the model. Pass `code` to replace \
                      the whole file first (a new part, or a genuine rewrite); omit it to run \
                      the file as it stands after `edit_script`. Returns the model's \
                      measurements and validity and anything the script printed, or the \
                      traceback to fix. The viewport shows the result immediately."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "The complete file, when replacing it. Omit to run the file as `edit_script` left it."},
                "summary": {"type": "string", "description": "One line saying what this version changes."}
            },
            "required": ["summary"],
            "additionalProperties": false
        }),
    }
}

pub fn edit_script_spec() -> ToolSpec {
    ToolSpec {
        name: EDIT_SCRIPT.to_string(),
        description: "Change part of the script in place: replace one exact occurrence of \
                      `old_text` with `new_text`. `old_text` must match the script exactly, \
                      whitespace included, and must be unique unless `replace_all` is true; \
                      include enough surrounding lines to make it so. Returns the edited \
                      region with line numbers. Nothing runs until `run_script`."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "old_text": {"type": "string", "description": "The exact text to replace, copied from the script."},
                "new_text": {"type": "string", "description": "The replacement text."},
                "replace_all": {"type": "boolean", "description": "Replace every occurrence instead of requiring exactly one."}
            },
            "required": ["old_text", "new_text"],
            "additionalProperties": false
        }),
    }
}

pub fn read_script_spec() -> ToolSpec {
    ToolSpec {
        name: READ_SCRIPT.to_string(),
        description: "Read the script with line numbers, whole or between two lines, as it \
                      now stands. Use it to see the lines a traceback names, or to copy exact \
                      text after an edit; the <current_script> block shows the file only as \
                      the turn began."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "start_line": {"type": "integer", "description": "First line to show, 1-based. Omit for the start of the file."},
                "end_line": {"type": "integer", "description": "Last line to show, inclusive. Omit for the end of the file."}
            },
            "additionalProperties": false
        }),
    }
}

pub fn run_python_spec() -> ToolSpec {
    ToolSpec {
        name: RUN_PYTHON.to_string(),
        description: "Run Python for what it prints and the value of its last expression, \
                      changing neither the script nor the model. By default the current \
                      script runs first and the snippet sees every name it bound — parts, \
                      sketches, parameters — so you can measure a face, list the edges a \
                      selector would pick, check a clearance, or try an API call before \
                      editing. Set `standalone` to skip the script for arithmetic or API \
                      probing. A traceback comes back as the result. Runs under the same \
                      time and memory limits as the script."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "The Python to run. End with an expression to get its value."},
                "standalone": {"type": "boolean", "description": "Run without the script first; the script's names are then absent."}
            },
            "required": ["code"],
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
                    "description": "Where the image is: {\"attachment\": {\"file\": ...}} for an image attached to a message in this conversation, by the file name shown in brackets after its name in the message's [Attached images: …] line; {\"library\": {\"file\": ...}} to re-describe a file already in the library.",
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {"attachment": {"type": "object", "properties": {"file": {"type": "string"}}, "required": ["file"], "additionalProperties": false}},
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
    let mut tools = vec![
        run_script_spec(),
        edit_script_spec(),
        read_script_spec(),
        run_python_spec(),
        lookup_docs_spec(),
    ];
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
        assert_eq!(run.code.as_deref(), Some("x = 1"));
        let rerun: RunScriptArgs =
            serde_json::from_value(json!({"summary": "Run the edited file"})).unwrap();
        assert_eq!(rerun.code, None);
        let edit: EditScriptArgs =
            serde_json::from_value(json!({"old_text": "x = 1", "new_text": "x = 2"})).unwrap();
        assert!(!edit.replace_all);
        let edit_all: EditScriptArgs =
            serde_json::from_value(json!({"old_text": "x", "new_text": "y", "replace_all": true}))
                .unwrap();
        assert!(edit_all.replace_all);
        let whole: ReadScriptArgs = serde_json::from_value(json!({})).unwrap();
        assert_eq!(whole, ReadScriptArgs::default());
        let range: ReadScriptArgs =
            serde_json::from_value(json!({"start_line": 10, "end_line": 20})).unwrap();
        assert_eq!((range.start_line, range.end_line), (Some(10), Some(20)));
        let snippet: RunPythonArgs =
            serde_json::from_value(json!({"code": "part.part.volume"})).unwrap();
        assert!(!snippet.standalone);
        let alone: RunPythonArgs =
            serde_json::from_value(json!({"code": "2 + 2", "standalone": true})).unwrap();
        assert!(alone.standalone);
        let render: RenderViewArgs = serde_json::from_value(json!({"view": "top"})).unwrap();
        assert_eq!(render.view, RenderView::Top);
        assert!(serde_json::from_value::<RenderViewArgs>(json!({"view": "sideways"})).is_err());
        let list: ReferenceImagesArgs = serde_json::from_value(json!({})).unwrap();
        assert_eq!(list.file, None);
        let read: ReferenceImagesArgs =
            serde_json::from_value(json!({"file": "flange.png"})).unwrap();
        assert_eq!(read.file.as_deref(), Some("flange.png"));
        let keep: KeepReferenceArgs = serde_json::from_value(json!({
            "source": {"attachment": {"file": "20260922-143000-flange.png"}},
            "file": "flange-top",
            "description": "Top view of the flange."
        }))
        .unwrap();
        assert_eq!(
            keep.source,
            KeepReferenceSource::Attachment {
                file: "20260922-143000-flange.png".into()
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
                EDIT_SCRIPT,
                READ_SCRIPT,
                RUN_PYTHON,
                LOOKUP_DOCS,
                RENDER_VIEW,
                REFERENCE_IMAGES,
                KEEP_REFERENCE
            ]
        );
        assert_eq!(
            names(false),
            [
                RUN_SCRIPT,
                EDIT_SCRIPT,
                READ_SCRIPT,
                RUN_PYTHON,
                LOOKUP_DOCS
            ]
        );
        let note = unavailable_tools_note(false).unwrap();
        assert!(note.contains(REFERENCE_IMAGES) && note.contains(KEEP_REFERENCE));
    }
}
