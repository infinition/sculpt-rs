//! Visual theme.
//!
//! One accent colour, a small neutral ramp, generous hit targets. The sizes are
//! driven by [`Theme::touch`] so the whole interface can grow for a touch screen
//! without any layout code caring.

use egui::{Color32, CornerRadius, Margin, Stroke, Vec2};

pub struct Palette;

impl Palette {
    pub const BG: Color32 = Color32::from_rgb(18, 19, 23);
    pub const PANEL: Color32 = Color32::from_rgb(25, 27, 32);
    pub const RAISED: Color32 = Color32::from_rgb(34, 37, 44);
    pub const HOVER: Color32 = Color32::from_rgb(45, 49, 58);
    pub const LINE: Color32 = Color32::from_rgb(52, 56, 66);
    pub const TEXT: Color32 = Color32::from_rgb(226, 229, 236);
    pub const DIM: Color32 = Color32::from_rgb(139, 146, 161);
    pub const FAINT: Color32 = Color32::from_rgb(96, 102, 116);
    pub const ACCENT: Color32 = Color32::from_rgb(255, 138, 76);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(122, 66, 38);
    pub const WARN: Color32 = Color32::from_rgb(240, 180, 90);
}

/// Metrics that scale with the touch setting.
#[derive(Clone, Copy)]
pub struct Metrics {
    pub tool: f32,
    pub button: f32,
    pub row: f32,
    pub gap: f32,
    pub pad: f32,
    pub radius: u8,
}

impl Metrics {
    pub fn for_touch(touch: bool) -> Self {
        if touch {
            Self { tool: 62.0, button: 50.0, row: 44.0, gap: 10.0, pad: 12.0, radius: 12 }
        } else {
            Self { tool: 46.0, button: 34.0, row: 30.0, gap: 6.0, pad: 8.0, radius: 8 }
        }
    }
}

/// Installs the theme on the context. Called every frame so a change to the
/// touch setting takes effect immediately.
pub fn apply(ctx: &egui::Context, touch: bool, ui_scale: f32) {
    let m = Metrics::for_touch(touch);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();

    style.spacing.item_spacing = Vec2::new(m.gap, m.gap * 0.8);
    style.spacing.button_padding = Vec2::new(m.pad, m.pad * 0.5);
    style.spacing.interact_size = Vec2::new(m.row, m.row);
    style.spacing.slider_width = 160.0;
    style.spacing.slider_rail_height = if touch { 12.0 } else { 8.0 };
    style.spacing.icon_width = m.row * 0.6;
    style.spacing.icon_width_inner = m.row * 0.38;
    style.spacing.window_margin = Margin::same(m.pad as i8);
    style.spacing.menu_margin = Margin::same(m.pad as i8);
    style.spacing.indent = 14.0;
    style.interaction.tooltip_delay = 0.35;
    style.interaction.resize_grab_radius_side = 12.0;
    style.animation_time = 0.09;

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.panel_fill = Palette::PANEL;
    v.window_fill = Palette::PANEL;
    v.extreme_bg_color = Palette::BG;
    v.faint_bg_color = Palette::RAISED;
    v.code_bg_color = Palette::BG;
    v.override_text_color = None;
    v.hyperlink_color = Palette::ACCENT;
    v.warn_fg_color = Palette::WARN;
    v.window_corner_radius = CornerRadius::same(m.radius);
    v.menu_corner_radius = CornerRadius::same(m.radius);
    v.window_stroke = Stroke::new(1.0, Palette::LINE);
    v.selection.bg_fill = Palette::ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0, Palette::ACCENT);
    v.slider_trailing_fill = true;
    v.striped = false;
    v.button_frame = true;

    let r = CornerRadius::same(m.radius);
    v.widgets.noninteractive.bg_fill = Palette::PANEL;
    v.widgets.noninteractive.weak_bg_fill = Palette::PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Palette::LINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Palette::DIM);
    v.widgets.noninteractive.corner_radius = r;

    v.widgets.inactive.bg_fill = Palette::RAISED;
    v.widgets.inactive.weak_bg_fill = Palette::RAISED;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, Palette::TEXT);
    v.widgets.inactive.corner_radius = r;

    v.widgets.hovered.bg_fill = Palette::HOVER;
    v.widgets.hovered.weak_bg_fill = Palette::HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Palette::LINE);
    v.widgets.hovered.fg_stroke = Stroke::new(1.2, Palette::TEXT);
    v.widgets.hovered.corner_radius = r;

    v.widgets.active.bg_fill = Palette::ACCENT_DIM;
    v.widgets.active.weak_bg_fill = Palette::ACCENT_DIM;
    v.widgets.active.bg_stroke = Stroke::new(1.0, Palette::ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.4, Palette::TEXT);
    v.widgets.active.corner_radius = r;

    v.widgets.open.bg_fill = Palette::RAISED;
    v.widgets.open.weak_bg_fill = Palette::RAISED;
    v.widgets.open.bg_stroke = Stroke::new(1.0, Palette::LINE);
    v.widgets.open.fg_stroke = Stroke::new(1.0, Palette::TEXT);
    v.widgets.open.corner_radius = r;

    use egui::{FontFamily, FontId, TextStyle};
    let base = if touch { 15.5 } else { 13.5 };
    style.text_styles = [
        (TextStyle::Small, FontId::new(base - 2.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(base, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(base, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(base + 4.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(base - 1.0, FontFamily::Monospace)),
    ]
    .into();

    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_zoom_factor(ui_scale);
}
