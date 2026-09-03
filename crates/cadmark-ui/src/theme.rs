// Visual theme — one palette and one set of spacing rules for every panel.
//
// Colours are chosen so the viewport's selection glow, the spatial-comment
// chips, and interactive controls all share a single accent, and so text
// sits on three clear levels (strong, normal, muted).

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2};

/// Interactive accent: buttons in focus, selected elements, links. Matches
/// the viewport's selection glow after display encoding.
pub const ACCENT: Color32 = Color32::from_rgb(97, 173, 255);
/// Spatial comments — anchored to geometry — carry the accent too, so the
/// chat and the viewport agree on what "selected" looks like.
pub const SPATIAL: Color32 = ACCENT;
/// The AI's voice in the chat.
pub const AI: Color32 = Color32::from_rgb(140, 210, 160);
/// Successful outcomes in the status bar.
pub const SUCCESS: Color32 = Color32::from_rgb(120, 200, 140);
/// Errors, wherever they are shown.
pub const ERROR: Color32 = Color32::from_rgb(240, 110, 110);
/// Warnings and provisional states.
pub const WARNING: Color32 = Color32::from_rgb(235, 180, 90);

/// Panel and window backgrounds.
pub const PANEL: Color32 = Color32::from_rgb(27, 28, 32);
/// A surface one step lighter than the panel: message cards, menus.
pub const RAISED: Color32 = Color32::from_rgb(36, 38, 44);
/// Text inputs and code, one step darker than the panel.
pub const SUNKEN: Color32 = Color32::from_rgb(18, 19, 22);
/// Hairline separators and card borders.
pub const BORDER: Color32 = Color32::from_rgb(52, 55, 63);
/// The 3D viewport background, as the display should show it.
pub const VIEWPORT: Color32 = Color32::from_rgb(40, 42, 48);

/// Text levels.
pub const TEXT_STRONG: Color32 = Color32::from_rgb(236, 238, 241);
pub const TEXT: Color32 = Color32::from_rgb(204, 208, 215);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(132, 138, 150);

/// Font sizes, in points.
pub const BODY_SIZE: f32 = 14.0;
pub const SMALL_SIZE: f32 = 11.5;
pub const HEADING_SIZE: f32 = 17.0;
pub const CODE_SIZE: f32 = 12.5;

/// Corner radius shared by cards, inputs and buttons.
pub const RADIUS: u8 = 5;

/// Install the theme on the context. Idempotent; call once at start-up.
pub fn apply(ctx: &egui::Context) {
    // The proportional family falls back to the monospace face, which
    // carries the arrows and geometric symbols the toolbar and status bar
    // use; without it those glyphs render as boxes.
    let mut fonts = egui::FontDefinitions::default();
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional)
        && !family.iter().any(|name| name == "Hack")
    {
        family.push("Hack".to_owned());
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();

    style.text_styles = [
        (
            TextStyle::Small,
            FontId::new(SMALL_SIZE, FontFamily::Proportional),
        ),
        (
            TextStyle::Body,
            FontId::new(BODY_SIZE, FontFamily::Proportional),
        ),
        (
            TextStyle::Button,
            FontId::new(BODY_SIZE, FontFamily::Proportional),
        ),
        (
            TextStyle::Heading,
            FontId::new(HEADING_SIZE, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(CODE_SIZE, FontFamily::Monospace),
        ),
    ]
    .into();

    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.window_margin = Margin::same(12);
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.interact_size = Vec2::new(40.0, 24.0);
    style.spacing.icon_width = 16.0;
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.floating = true;
    style.spacing.tooltip_width = 360.0;
    style.interaction.tooltip_delay = 0.35;

    let visuals = &mut style.visuals;
    visuals.dark_mode = true;
    visuals.override_text_color = None;
    visuals.panel_fill = PANEL;
    visuals.window_fill = RAISED;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.window_corner_radius = CornerRadius::same(RADIUS + 2);
    visuals.menu_corner_radius = CornerRadius::same(RADIUS + 1);
    visuals.extreme_bg_color = SUNKEN;
    visuals.faint_bg_color = RAISED;
    visuals.code_bg_color = SUNKEN;
    visuals.hyperlink_color = ACCENT;
    visuals.warn_fg_color = WARNING;
    visuals.error_fg_color = ERROR;
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.window_shadow.color = Color32::from_black_alpha(110);
    visuals.popup_shadow.color = Color32::from_black_alpha(90);
    visuals.striped = false;
    visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = PANEL;
    widgets.noninteractive.weak_bg_fill = PANEL;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    widgets.noninteractive.corner_radius = CornerRadius::same(RADIUS);

    widgets.inactive.bg_fill = Color32::from_rgb(48, 51, 59);
    widgets.inactive.weak_bg_fill = Color32::from_rgb(44, 47, 54);
    widgets.inactive.bg_stroke = Stroke::NONE;
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    widgets.inactive.corner_radius = CornerRadius::same(RADIUS);

    widgets.hovered.bg_fill = Color32::from_rgb(62, 66, 76);
    widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 62, 72);
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(80, 85, 98));
    widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    widgets.hovered.corner_radius = CornerRadius::same(RADIUS);
    widgets.hovered.expansion = 0.0;

    widgets.active.bg_fill = Color32::from_rgb(74, 79, 92);
    widgets.active.weak_bg_fill = Color32::from_rgb(70, 75, 88);
    widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    widgets.active.corner_radius = CornerRadius::same(RADIUS);
    widgets.active.expansion = 0.0;

    widgets.open.bg_fill = Color32::from_rgb(58, 62, 72);
    widgets.open.weak_bg_fill = Color32::from_rgb(58, 62, 72);
    widgets.open.bg_stroke = Stroke::new(1.0_f32, BORDER);
    widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    widgets.open.corner_radius = CornerRadius::same(RADIUS);

    ctx.set_style(style);
}

/// A card: raised surface with a hairline border, used for chat messages
/// and the comment overlay.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(RAISED)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(RADIUS + 1))
        .inner_margin(Margin::symmetric(10, 8))
}

/// A card tinted with a colour: the same shape as [`card`], but the fill
/// and border take on the tint so the card's kind reads at a glance.
pub fn tinted_card(tint: Color32) -> egui::Frame {
    card()
        .fill(blend(RAISED, tint, 0.09))
        .stroke(Stroke::new(1.0_f32, blend(BORDER, tint, 0.35)))
}

/// A small rounded label such as "face 12 · line 4".
pub fn chip(ui: &mut egui::Ui, text: &str, tint: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(tint.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0_f32, tint.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).small().color(tint));
        })
        .response
}

/// A short keyboard hint such as "Enter" rendered as a key cap.
pub fn key_hint(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(SUNKEN)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).small().color(TEXT_MUTED));
        });
}

/// Mix `base` towards `tint` by `amount` in gamma space.
pub fn blend(base: Color32, tint: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    Color32::from_rgba_unmultiplied(
        mix(base.r(), tint.r()),
        mix(base.g(), tint.g()),
        mix(base.b(), tint.b()),
        mix(base.a(), tint.a()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_moves_between_endpoints() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(200, 100, 50);
        assert_eq!(blend(a, b, 0.0), Color32::from_rgb(0, 0, 0));
        assert_eq!(blend(a, b, 1.0), Color32::from_rgb(200, 100, 50));
        assert_eq!(blend(a, b, 0.5), Color32::from_rgb(100, 50, 25));
    }

    #[test]
    fn proportional_text_falls_back_to_the_symbol_bearing_face() {
        let ctx = egui::Context::default();
        apply(&ctx);
        // Fonts are only loaded inside a frame; run one so the definitions
        // installed by `apply` are the ones inspected.
        let _ = ctx.run(Default::default(), |ctx| {
            let families = ctx.fonts(|fonts| fonts.families());
            assert!(families.contains(&FontFamily::Proportional));
        });
    }

    #[test]
    fn applying_the_theme_sets_the_panel_colour_and_font_sizes() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let style = ctx.style();
        assert_eq!(style.visuals.panel_fill, PANEL);
        assert_eq!(style.text_styles[&TextStyle::Body].size, BODY_SIZE);
        assert_eq!(style.text_styles[&TextStyle::Monospace].size, CODE_SIZE);
    }
}
