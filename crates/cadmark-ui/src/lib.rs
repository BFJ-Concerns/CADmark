// CADmark UI — egui panels and application shell.
//
// A shared theme, the toolbar, the chat pane with message type
// distinction, the comment overlay positioned near selected geometry,
// the view gizmo in the viewport corner, the read-only code panel,
// the parameters panel, the status bar, the start view shown before a project is chosen, the
// part-naming prompt, the version-naming dialog, and the settings dialog.

pub mod chat;
pub mod code_panel;
pub mod overlay;
pub mod parameters;
pub mod part_name_dialog;
pub mod settings_dialog;
pub mod start_view;
pub mod status;
mod text_input;
pub mod theme;
pub mod toolbar;
pub mod version_dialog;
pub mod view_gizmo;
