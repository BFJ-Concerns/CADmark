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
/// The tool that details a run of the current model's elements.
pub const INSPECT_ELEMENTS: &str = "inspect_elements";

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

/// Which kind of element `inspect_elements` is being asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    Face,
    Edge,
    Vertex,
}

/// Arguments of `inspect_elements`: one run of IDs of one kind, written
/// the way a run result lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectElementsArgs {
    pub kind: ElementKind,
    /// The first ID wanted.
    pub first: u32,
    /// The last ID wanted; one element when absent.
    #[serde(default)]
    pub last: Option<u32>,
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

pub fn inspect_elements_spec() -> ToolSpec {
    ToolSpec {
        name: INSPECT_ELEMENTS.to_string(),
        description: "Get the source line and measurements of a run of the current model's \
                      elements, by ID. Use when a run result listed elements as ID ranges \
                      rather than singly and you need to tell one element of an operation \
                      from another before naming it in your reply."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": ["face", "edge", "vertex"]},
                "first": {"type": "integer", "minimum": 0, "description": "The first ID wanted."},
                "last": {"type": "integer", "minimum": 0, "description": "The last ID wanted; omit for one element."}
            },
            "required": ["kind", "first"],
            "additionalProperties": false
        }),
    }
}

/// The tools offered for one turn. The render tool is offered only to a
/// model that reads images; a text-only model is told so in the
/// instructions rather than handed a tool it cannot use.
pub fn tools_for(accepts_images: bool) -> Vec<ToolSpec> {
    let mut tools = vec![
        run_script_spec(),
        lookup_docs_spec(),
        inspect_elements_spec(),
    ];
    if accepts_images {
        tools.push(render_view_spec());
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_arguments_parse_from_the_models_json() {
        let run: RunScriptArgs =
            serde_json::from_value(json!({"code": "x = 1", "summary": "Set x"})).unwrap();
        assert_eq!(run.code, "x = 1");
        let render: RenderViewArgs = serde_json::from_value(json!({"view": "top"})).unwrap();
        assert_eq!(render.view, RenderView::Top);
        assert!(serde_json::from_value::<RenderViewArgs>(json!({"view": "sideways"})).is_err());
        let run: InspectElementsArgs =
            serde_json::from_value(json!({"kind": "edge", "first": 40, "last": 79})).unwrap();
        assert_eq!(run.kind, ElementKind::Edge);
        assert_eq!((run.first, run.last), (40, Some(79)));
        // One element is a run with no end.
        let one: InspectElementsArgs =
            serde_json::from_value(json!({"kind": "face", "first": 3})).unwrap();
        assert_eq!(
            (one.kind, one.first, one.last),
            (ElementKind::Face, 3, None)
        );
        assert!(
            serde_json::from_value::<InspectElementsArgs>(json!({"kind": "loop", "first": 1}))
                .is_err()
        );
    }

    #[test]
    fn the_render_tool_is_withheld_from_text_only_models() {
        let names = |accepts: bool| {
            tools_for(accepts)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(true),
            [RUN_SCRIPT, LOOKUP_DOCS, INSPECT_ELEMENTS, RENDER_VIEW]
        );
        assert_eq!(names(false), [RUN_SCRIPT, LOOKUP_DOCS, INSPECT_ELEMENTS]);
    }
}
