//! Visual theme.
//!
//! Everything the interface looks like is derived from a handful of user
//! settings in [`UiTheme`]: two colours, a few sizes, and where each dock goes.
//! The derived [`Palette`] is stashed in the egui context once per frame so any
//! widget can read it without threading a reference through every signature.

use egui::{Color32, CornerRadius, Margin, Stroke, Vec2};

/// Where a dock sits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];
    pub const SIDES: [Side; 2] = [Side::Left, Side::Right];

    pub fn label(self) -> &'static str {
        match self {
            Side::Left => "Left",
            Side::Right => "Right",
            Side::Top => "Top",
            Side::Bottom => "Bottom",
        }
    }

    pub fn is_horizontal(self) -> bool {
        matches!(self, Side::Top | Side::Bottom)
    }
}

/// A named starting point for the colour scheme.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorPreset {
    Cyan,
    Amber,
    Violet,
    Lime,
    Rose,
}

impl ColorPreset {
    pub const ALL: [ColorPreset; 5] = [
        ColorPreset::Cyan,
        ColorPreset::Amber,
        ColorPreset::Violet,
        ColorPreset::Lime,
        ColorPreset::Rose,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ColorPreset::Cyan => "Cyan",
            ColorPreset::Amber => "Amber",
            ColorPreset::Violet => "Violet",
            ColorPreset::Lime => "Lime",
            ColorPreset::Rose => "Rose",
        }
    }

    pub fn accent(self) -> [f32; 3] {
        match self {
            ColorPreset::Cyan => [0.13, 0.85, 1.0],
            ColorPreset::Amber => [1.0, 0.62, 0.25],
            ColorPreset::Violet => [0.66, 0.48, 1.0],
            ColorPreset::Lime => [0.55, 0.92, 0.35],
            ColorPreset::Rose => [1.0, 0.42, 0.58],
        }
    }
}

/// Everything the user can change about the look and the layout.
#[derive(Clone, Copy, PartialEq)]
pub struct UiTheme {
    /// Electric accent, used for selection, handles and highlights.
    pub accent: [f32; 3],
    /// Darkest neutral; every other grey is derived from it.
    pub base: [f32; 3],
    /// How far the greys spread from the base, 0 flat to 1 contrasty.
    pub contrast: f32,
    pub text: [f32; 3],
    /// Global egui zoom.
    pub ui_scale: f32,
    /// Extra multiplier on glyph size inside buttons.
    pub icon_scale: f32,
    /// Extra multiplier on every font.
    pub text_scale: f32,
    /// Edge of a tool tile, before scaling.
    pub tool_size: f32,
    /// Height of a slider, toggle or button row.
    pub row_height: f32,
    /// Width of the settings dock.
    pub panel_width: f32,
    pub corner_radius: u8,
    /// Bumps every target up to a size a fingertip can hit.
    pub touch: bool,
    pub rail_side: Side,
    pub panel_side: Side,
    /// Width of the scroll bars.
    pub scrollbar: f32,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            accent: ColorPreset::Cyan.accent(),
            base: [0.086, 0.090, 0.098],
            contrast: 0.55,
            text: [0.88, 0.90, 0.93],
            ui_scale: 1.0,
            icon_scale: 1.0,
            text_scale: 1.0,
            tool_size: 46.0,
            row_height: 30.0,
            panel_width: 300.0,
            corner_radius: 9,
            touch: false,
            rail_side: Side::Left,
            panel_side: Side::Right,
            scrollbar: 10.0,
        }
    }
}

impl UiTheme {
    /// The multiplier applied to every size when touch mode is on.
    fn size_boost(&self) -> f32 {
        if self.touch { 1.38 } else { 1.0 }
    }
}

fn rgb(c: [f32; 3]) -> Color32 {
    Color32::from_rgb(
        (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// Lifts a colour toward white by `t`, which is how the neutral ramp is built.
fn lift(c: [f32; 3], t: f32) -> [f32; 3] {
    [
        c[0] + (1.0 - c[0]) * t,
        c[1] + (1.0 - c[1]) * t,
        c[2] + (1.0 - c[2]) * t,
    ]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Colours derived from the theme, ready to paint with.
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub raised: Color32,
    pub hover: Color32,
    pub line: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_dim: Color32,
    pub accent_soft: Color32,
    pub warn: Color32,
}

impl Palette {
    pub fn derive(t: &UiTheme) -> Self {
        let k = t.contrast.clamp(0.0, 1.0);
        let base = t.base;
        Self {
            bg: rgb(mix(base, lift(base, -0.0), 1.0)),
            panel: rgb(lift(base, 0.045 * k + 0.02)),
            raised: rgb(lift(base, 0.10 * k + 0.03)),
            hover: rgb(lift(base, 0.17 * k + 0.05)),
            line: rgb(lift(base, 0.22 * k + 0.04)),
            text: rgb(t.text),
            dim: rgb(mix(t.text, base, 0.42)),
            faint: rgb(mix(t.text, base, 0.66)),
            accent: rgb(t.accent),
            // A dark, saturated version of the accent for filled states.
            accent_dim: rgb(mix(base, t.accent, 0.34)),
            accent_soft: rgb(mix(base, t.accent, 0.16)),
            warn: rgb([0.98, 0.72, 0.35]),
        }
    }

    /// The palette installed for this frame.
    pub fn of(ctx: &egui::Context) -> Palette {
        ctx.data(|d| d.get_temp::<Palette>(egui::Id::new("sculpt_palette")))
            .unwrap_or_else(|| Palette::derive(&UiTheme::default()))
    }

    /// Convenience for widgets, which always have a `Ui` at hand.
    pub fn ui(ui: &egui::Ui) -> Palette {
        Palette::of(ui.ctx())
    }
}

/// Sizes derived from the theme.
#[derive(Clone, Copy)]
pub struct Metrics {
    pub tool: f32,
    pub button: f32,
    pub row: f32,
    pub gap: f32,
    pub pad: f32,
    pub radius: u8,
    pub panel_width: f32,
}

impl Metrics {
    pub fn derive(t: &UiTheme) -> Self {
        let boost = t.size_boost();
        let row = (t.row_height * boost).clamp(22.0, 96.0);
        Self {
            tool: (t.tool_size * boost).clamp(30.0, 130.0),
            button: (t.row_height * boost * 1.16).clamp(24.0, 104.0),
            row,
            gap: (row * 0.2).clamp(4.0, 14.0),
            pad: (row * 0.28).clamp(6.0, 18.0),
            radius: t.corner_radius,
            panel_width: (t.panel_width * boost).clamp(220.0, 640.0),
        }
    }
}

/// Installs the theme on the context. Called every frame so a change to any
/// setting takes effect immediately.
pub fn apply(ctx: &egui::Context, t: &UiTheme) -> (Palette, Metrics) {
    let p = Palette::derive(t);
    let m = Metrics::derive(t);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("sculpt_palette"), p));

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();

    style.spacing.item_spacing = Vec2::new(m.gap, m.gap * 0.8);
    style.spacing.button_padding = Vec2::new(m.pad, m.pad * 0.5);
    style.spacing.interact_size = Vec2::new(m.row, m.row);
    style.spacing.slider_width = 160.0;
    style.spacing.slider_rail_height = m.row * 0.34;
    style.spacing.icon_width = m.row * 0.6;
    style.spacing.icon_width_inner = m.row * 0.38;
    style.spacing.window_margin = Margin::same(m.pad as i8);
    style.spacing.menu_margin = Margin::same(m.pad as i8);
    style.spacing.indent = 14.0;
    style.interaction.tooltip_delay = 0.35;
    style.interaction.resize_grab_radius_side = if t.touch { 16.0 } else { 10.0 };
    style.animation_time = 0.09;

    // Scroll bars: always visible, wide enough to grab, and coloured like the
    // rest of the interface rather than egui's defaults.
    let bar = (t.scrollbar * t.size_boost()).clamp(6.0, 26.0);
    let s = &mut style.spacing.scroll;
    s.floating = false;
    s.bar_width = bar;
    s.handle_min_length = 28.0;
    s.bar_inner_margin = 2.0;
    s.bar_outer_margin = 0.0;
    s.foreground_color = true;

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.bg;
    v.faint_bg_color = p.raised;
    v.code_bg_color = p.bg;
    v.override_text_color = None;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.window_corner_radius = CornerRadius::same(m.radius);
    v.menu_corner_radius = CornerRadius::same(m.radius);
    v.window_stroke = Stroke::new(1.0, p.line);
    v.selection.bg_fill = p.accent_dim;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.slider_trailing_fill = true;
    v.striped = false;
    v.button_frame = true;
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.window_shadow = v.popup_shadow;

    let r = CornerRadius::same(m.radius);
    v.widgets.noninteractive.bg_fill = p.panel;
    v.widgets.noninteractive.weak_bg_fill = p.panel;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.dim);
    v.widgets.noninteractive.corner_radius = r;

    v.widgets.inactive.bg_fill = p.raised;
    v.widgets.inactive.weak_bg_fill = p.raised;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.corner_radius = r;

    v.widgets.hovered.bg_fill = p.hover;
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.hovered.fg_stroke = Stroke::new(1.2, p.text);
    v.widgets.hovered.corner_radius = r;

    v.widgets.active.bg_fill = p.accent_dim;
    v.widgets.active.weak_bg_fill = p.accent_dim;
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.4, p.text);
    v.widgets.active.corner_radius = r;

    v.widgets.open.bg_fill = p.raised;
    v.widgets.open.weak_bg_fill = p.raised;
    v.widgets.open.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.open.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.open.corner_radius = r;

    use egui::{FontFamily, FontId, TextStyle};
    let base = 13.5 * t.text_scale.clamp(0.6, 2.0) * if t.touch { 1.14 } else { 1.0 };
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
    ctx.set_zoom_factor(t.ui_scale.clamp(0.6, 2.5));
    (p, m)
}
