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
        description: "Look up build123d API signatures and usage from its documentation. Use \
                      before an operation you are not certain of."
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

/// The tools offered for one turn. The render tool is offered only to a
/// model that reads images; a text-only model is told so in the
/// instructions rather than handed a tool it cannot use.
pub fn tools_for(accepts_images: bool) -> Vec<ToolSpec> {
    let mut tools = vec![run_script_spec(), lookup_docs_spec()];
    if accepts_images {
        tools.push(render_view_spec());
    }
    tools
}

/// What a model is told about a tool it is not being offered. A model
/// that cannot read images would otherwise have to infer the render
/// tool's absence from a list it never sees in full, so the instructions
/// say it plainly instead.
pub fn unavailable_tools_note(accepts_images: bool) -> Option<&'static str> {
    (!accepts_images).then_some(
        "The `render_view` tool is unavailable in this session: the model in use cannot read \
         images. Judge the result from the measurements and validity the run reports, and say \
         so when you cannot check it by eye.",
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
    }

    #[test]
    fn the_render_tool_is_withheld_from_text_only_models() {
        let names = |accepts: bool| {
            tools_for(accepts)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(true), [RUN_SCRIPT, LOOKUP_DOCS, RENDER_VIEW]);
        assert_eq!(names(false), [RUN_SCRIPT, LOOKUP_DOCS]);
    }
}
