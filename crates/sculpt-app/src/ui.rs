//! The interface: top bar, brush rail, settings dock, viewport overlays.
//!
//! Layout is built around one idea: the tool you are holding is always one
//! glance away, its settings are always in the same dock, and everything you
//! can touch is at least a fingertip wide. Where those docks sit, how big they
//! are and what colour everything is are all yours to change from the UI tab.

use crate::camera::{Camera, Projection, ViewPreset};
use crate::gizmo::{Gizmo, GizmoMode};
use crate::hud::{Hud, HudAction};
use crate::icons::Icon;
use crate::input::{Bindings, Role};
use crate::matcap;
use crate::navwidget::{self, NavAction, NavWidget};
use crate::renderer::{FrameSettings, Shading};
use crate::theme::{ColorPreset, Metrics, Palette, Side, UiTheme};
use crate::wheel::{self, Wheel};
use crate::widgets::{self, BigSlider};
use egui::{Align2, Color32, CornerRadius, Frame, Margin, Sense, Stroke, Vec2};
use sculpt_core::{io, Axis, BlendMode, BrushKind, Falloff, FillScope, RemeshOptions, Sculptor};
use winit::keyboard::KeyCode;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Primitive {
    Sphere,
    UvSphere,
    Cube,
    Cylinder,
    Torus,
    Plane,
}

impl Primitive {
    pub const ALL: [Primitive; 6] = [
        Primitive::Sphere,
        Primitive::UvSphere,
        Primitive::Cube,
        Primitive::Cylinder,
        Primitive::Torus,
        Primitive::Plane,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Primitive::Sphere => "Sphere",
            Primitive::UvSphere => "UV sphere",
            Primitive::Cube => "Cube",
            Primitive::Cylinder => "Cylinder",
            Primitive::Torus => "Torus",
            Primitive::Plane => "Plane",
        }
    }
}

#[derive(Debug)]
pub enum Action {
    New(Primitive),
    AddObject(Primitive),
    Import,
    ImportObject,
    Export,
    OpenScene,
    SaveScene,
    Undo,
    Redo,
    ClearMask,
    InvertMask,
    FilterMask(f32),
    ExtractMask,
    FrameView,
    MatcapChanged,
    SampleCountChanged(u32),
    Subdivide(bool),
    Decimate,
    Remesh,
    CloseHoles,
    SmoothAll,
    Mirror(Axis),
    Symmetrize(Axis, bool),
    SelectObject(usize),
    DeleteObject(usize),
    DuplicateObject(usize),
    ToggleVisible(usize),
    MergeVisible,
    ApplyTransform,
    SetView(ViewPreset),
    PickColorMode,
    LoadAlpha,
    LoadMatcap,
    SaveBrushes,
    LoadBrushes,
    ResetBrushes,
    ResetTheme,
    SaveWorkspace,
    LoadBrushIcon(usize),
    /// Toggle voxel sculpting: the brush stamps a voxel field instead of the
    /// mesh, so the cost of a dab stops depending on the polygon count.
    ToggleVoxel,
}

/// Where things that pop up should appear.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// Wherever the pointer is, which is where you are already looking.
    Cursor,
    /// The middle of the viewport, which never moves and never lands under a
    /// dock. Easier to reach on a tablet held in two hands.
    Center,
}

impl Anchor {
    pub const ALL: [Anchor; 2] = [Anchor::Cursor, Anchor::Center];

    pub fn label(self) -> &'static str {
        match self {
            Anchor::Cursor => "At the cursor",
            Anchor::Center => "In the middle",
        }
    }

    /// Resolves to a point, given where the pointer is and what the viewport is.
    pub fn resolve(self, cursor: egui::Pos2, viewport: egui::Rect) -> egui::Pos2 {
        match self {
            Anchor::Cursor => cursor,
            Anchor::Center => viewport.center(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tab {
    Brush,
    Colour,
    Materials,
    Model,
    Scene,
    View,
    Interface,
}

impl Tab {
    const ALL: [Tab; 7] = [
        Tab::Brush,
        Tab::Colour,
        Tab::Materials,
        Tab::Model,
        Tab::Scene,
        Tab::View,
        Tab::Interface,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Brush => "Brush",
            Tab::Colour => "Colour",
            Tab::Materials => "Materials",
            Tab::Model => "Model",
            Tab::Scene => "Scene",
            Tab::View => "View",
            Tab::Interface => "UI",
        }
    }
}

pub struct UiState {
    pub theme: UiTheme,
    pub matcap: matcap::Preset,
    pub settings: FrameSettings,
    pub tab: Tab,
    pub show_panel: bool,
    pub show_rail: bool,
    pub show_stats: bool,
    pub show_help: bool,
    /// What the last frames cost to produce, in milliseconds, smoothed.
    ///
    /// The honest number: the rate a frame would sustain if one were asked for
    /// straight after the last. The gap between frames is not that, because at
    /// rest nothing asks for one.
    pub frame_ms: f32,
    pub status: String,
    pub status_age: f32,
    pub remesh: RemeshOptions,
    pub decimate_ratio: f32,
    pub extract_thickness: f32,
    pub subdivide_smooth: bool,
    pub sample_count: u32,
    pub msaa_available: bool,
    /// Highest sample count this GPU accepts for both colour and depth.
    pub max_samples: u32,
    pub wireframe_available: bool,
    /// Turn multisampling off by itself once the triangles are smaller than a
    /// pixel, where it costs four times the rasterising for no visible gain.
    pub adaptive_msaa: bool,
    pub picking_color: bool,
    /// Colour under the cursor while the eyedropper is armed. Sampled off the
    /// model as the pointer moves, so what a click would take is on screen
    /// before the click.
    pub hover_color: Option<glam::Vec3>,
    pub add_primitive: Primitive,
    /// How far past the silhouette a stroke may reach for the surface, in
    /// interface points. Zero means the cursor must be on the model.
    pub brush_reach: f32,
    /// Bytes the last frame sent to the GPU, shown in the statistics.
    pub upload_bytes: u64,
    /// Faces actually sent to the GPU last frame, and how many draw calls it
    /// took. Shown in the statistics: the whole point of the partition is that
    /// the first number is far below the triangle count, and there is no way to
    /// tell without looking.
    pub drawn_faces: u32,
    pub draw_calls: u32,
    pub nav: NavWidget,
    pub show_nav: bool,
    pub hud: Hud,
    pub bindings: Bindings,
    pub wheel: Wheel,
    /// Where the radial menu and the brush preview appear.
    pub anchor: Anchor,
    /// The viewport as it was last frame, so code outside the interface can
    /// place things in it.
    pub viewport: egui::Rect,
    /// Last pointer position that was over the model rather than over the
    /// interface. While a slider is being dragged the pointer is on the
    /// slider, and drawing the brush preview there would be useless, so this
    /// remembers where the brush actually was.
    pub viewport_cursor: egui::Pos2,
    /// Key that summons the radial menu while held.
    pub wheel_key: KeyCode,
    /// True while waiting for the user to press the key they want.
    pub rebinding_wheel: bool,
    /// Interface scale being dragged, applied when the drag ends. Zooming
    /// rescales the coordinate space the slider itself lives in, so committing
    /// mid-gesture would move the rail out from under the finger.
    pub ui_scale_draft: f32,
    /// One thumbnail per alpha, in the same order. Built once and rebuilt only
    /// when the library changes: an alpha is a full image, and uploading them
    /// every frame to draw a row of postage stamps would be absurd.
    pub alpha_thumbs: Vec<egui::TextureHandle>,
    /// Where the matcap comes from.
    pub matcap_source: MatcapSource,
    /// The material and lights behind the generated matcap, editable.
    pub lightcap: matcap::Lightcap,
    /// A matcap loaded from a file: side length and RGBA8 pixels.
    pub matcap_image: Option<(u32, Vec<u8>)>,
    /// Tabs that have been torn out of the dock and float over the viewport.
    pub floating: Vec<Tab>,
    /// Scheme the colour panel shows alongside the colour in hand.
    pub harmony: Harmony,
    /// Brushes the artist built and named.
    pub brushes: Vec<io::NamedBrush>,
    /// Name being typed for the next one kept.
    pub brush_name: String,
    pub materials: Vec<crate::materials::Material>,
    pub material_name: String,
    pub preview_opacity: f32,
    pub(crate) preview_texture: Option<egui::TextureHandle>,
    pub(crate) preview_key: Option<(Falloff, Option<u32>, usize)>,
    pub(crate) brush_icons: std::collections::HashMap<String, egui::TextureHandle>,
}

/// Whether a press at this point belongs to the interface rather than the model.
///
/// Asking egui whether it wants the pointer is not enough at the moment of
/// contact. That answer is built from the previous frame, and it is built from
/// hovering: a mouse has to travel over a button before it can press it, so by
/// the time the press lands egui already knows. A pen and a finger have no
/// hover at all. Their first report is the press itself, egui has never seen
/// the pointer there, and the press reads as landing on the model, which is how
/// dragging the zoom button also spun the view.
///
/// So the question is asked of the geometry instead: outside the viewport is a
/// dock, and anything egui has put in a layer above the background inside it is
/// a floating control or a window.
pub fn interface_owns(ctx: &egui::Context, viewport: egui::Rect, at: egui::Pos2) -> bool {
    if !viewport.contains(at) {
        return true;
    }
    matches!(ctx.layer_id_at(at), Some(layer) if layer.order > egui::Order::Background)
}

/// Colour schemes built off the hue in hand.
///
/// The point is not the arithmetic, which is trivial, but having the other
/// colours already on screen: picking a shadow by eye from a hue wheel is how
/// paintings end up muddy.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Harmony {
    None,
    Complement,
    Analogous,
    Triad,
    Split,
    Shades,
}

impl Harmony {
    pub const ALL: [Harmony; 6] = [
        Harmony::None,
        Harmony::Complement,
        Harmony::Analogous,
        Harmony::Triad,
        Harmony::Split,
        Harmony::Shades,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Harmony::None => "Off",
            Harmony::Complement => "Opposite",
            Harmony::Analogous => "Near",
            Harmony::Triad => "Triad",
            Harmony::Split => "Split",
            Harmony::Shades => "Shades",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Harmony::None => "",
            Harmony::Complement => "The hue across the wheel. Loud, and the fastest way to make one colour read.",
            Harmony::Analogous => "The neighbours. Quiet, and what most of a painting is made of.",
            Harmony::Triad => "Three evenly spaced hues. Balanced without being flat.",
            Harmony::Split => "The two either side of the opposite. Most of the contrast, less of the shouting.",
            Harmony::Shades => "The same hue at other values, which is what a form needs before it needs another hue.",
        }
    }

    /// The colours this scheme puts alongside the one in hand.
    pub fn mates(self, hue: f32, sat: f32, val: f32) -> Vec<glam::Vec3> {
        let at = |h: f32, s: f32, v: f32| {
            let (r, g, b) = wheel::hsv_to_rgb(h.rem_euclid(1.0), s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
            glam::Vec3::new(r, g, b)
        };
        match self {
            Harmony::None => Vec::new(),
            Harmony::Complement => vec![at(hue + 0.5, sat, val)],
            Harmony::Analogous => vec![at(hue - 1.0 / 12.0, sat, val), at(hue + 1.0 / 12.0, sat, val)],
            Harmony::Triad => vec![at(hue + 1.0 / 3.0, sat, val), at(hue - 1.0 / 3.0, sat, val)],
            Harmony::Split => vec![at(hue + 0.5 - 1.0 / 12.0, sat, val), at(hue + 0.5 + 1.0 / 12.0, sat, val)],
            // Down toward the shadow, up toward the light, and a little less
            // saturated at both ends the way pigment behaves.
            Harmony::Shades => vec![
                at(hue, sat * 0.9, val * 0.45),
                at(hue, sat * 0.95, val * 0.7),
                at(hue, sat * 0.8, (val * 1.25).min(1.0)),
                at(hue, sat * 0.55, (val * 1.5).min(1.0)),
            ],
        }
    }
}

/// The three ways to get a matcap.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum MatcapSource {
    /// One of the built-in materials, lit the built-in way.
    Preset,
    /// The same thing taken apart: material and three lights, all editable.
    Lightcap,
    /// An image someone else baked.
    Image,
}

impl MatcapSource {
    pub const ALL: [MatcapSource; 3] =
        [MatcapSource::Preset, MatcapSource::Lightcap, MatcapSource::Image];

    pub fn label(self) -> &'static str {
        match self {
            MatcapSource::Preset => "Preset",
            MatcapSource::Lightcap => "Lights",
            MatcapSource::Image => "Image",
        }
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            theme: UiTheme::default(),
            matcap: matcap::Preset::Clay,
            settings: FrameSettings::default(),
            tab: Tab::Brush,
            show_panel: true,
            show_rail: true,
            show_stats: true,
            show_help: false,
            frame_ms: 0.0,
            status: String::new(),
            status_age: 0.0,
            remesh: RemeshOptions::default(),
            decimate_ratio: 0.5,
            extract_thickness: 0.05,
            subdivide_smooth: true,
            sample_count: 4,
            msaa_available: true,
            max_samples: 4,
            wireframe_available: true,
            adaptive_msaa: true,
            picking_color: false,
            hover_color: None,
            add_primitive: Primitive::Sphere,
            brush_reach: 10.0,
            upload_bytes: 0,
            drawn_faces: 0,
            draw_calls: 0,
            nav: NavWidget::default(),
            show_nav: true,
            hud: Hud::default(),
            bindings: Bindings::default(),
            wheel: Wheel::default(),
            anchor: Anchor::Cursor,
            viewport: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0)),
            viewport_cursor: egui::pos2(640.0, 360.0),
            wheel_key: KeyCode::Space,
            rebinding_wheel: false,
            ui_scale_draft: UiTheme::default().ui_scale,
            alpha_thumbs: Vec::new(),
            matcap_source: MatcapSource::Preset,
            lightcap: matcap::Lightcap::default(),
            matcap_image: None,
            floating: Vec::new(),
            harmony: Harmony::Analogous,
            brushes: Vec::new(),
            brush_name: String::new(),
            materials: crate::materials::defaults(),
            material_name: String::new(),
            preview_opacity: 0.28,
            preview_texture: None,
            preview_key: None,
            brush_icons: Default::default(),
        }
    }
}

impl UiState {
    pub fn say(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_age = 0.0;
    }

    /// Makes sure there is one thumbnail per alpha.
    ///
    /// The thumbnails are white with the alpha in their opacity, so the grid
    /// can tint them with the theme and a selected one lights up in the accent
    /// colour without a second texture.
    fn sync_alpha_thumbs(&mut self, ctx: &egui::Context, s: &Sculptor) {
        if self.alpha_thumbs.len() == s.alphas.len() {
            return;
        }
        self.alpha_thumbs.clear();
        const SIDE: usize = 48;
        for (i, a) in s.alphas.iter().enumerate() {
            let mut pixels = Vec::with_capacity(SIDE * SIDE);
            for y in 0..SIDE {
                for x in 0..SIDE {
                    let v = a.sample(x as f32 / (SIDE - 1) as f32, y as f32 / (SIDE - 1) as f32);
                    pixels.push(egui::Color32::from_white_alpha((v * 255.0) as u8));
                }
            }
            let image = egui::ColorImage {
                size: [SIDE, SIDE],
                pixels,
                source_size: egui::vec2(SIDE as f32, SIDE as f32),
            };
            self.alpha_thumbs.push(ctx.load_texture(
                format!("alpha_{i}"),
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
    }
}

/// What the viewport should paint on top of the 3D image.
pub struct Overlay {
    /// Cursor centre and radius, in points.
    pub cursor: Option<(egui::Pos2, f32)>,
    /// Surface normal under the cursor, projected to screen, for the tilt spoke.
    pub cursor_tilt: Option<(f32, f32)>,
    /// Tangent-plane stamp, clockwise from the upper left. One surface pick.
    pub footprint: Option<[egui::Pos2; 4]>,
    /// Where the brush will actually bite, when that is not under the cursor.
    /// Set while the cursor sits off the silhouette and the stroke is reaching
    /// for the nearest surface instead.
    pub anchor: Option<egui::Pos2>,
    pub stroking: bool,
}

/// Everything one panel body needs.
struct Ctx<'a> {
    p: Palette,
    m: Metrics,
    actions: &'a mut Vec<Action>,
}

/// Builds the whole interface for one frame and returns the actions it raised.
pub fn draw(
    root: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    cam: &mut Camera,
    giz: &mut Gizmo,
    overlay: &Overlay,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let p = Palette::ui(root);
    let m = Metrics::derive(&st.theme);

    {
        let mut cx = Ctx { p, m, actions: &mut actions };
        top_bar(root, s, st, cam, giz, &mut cx);
        if st.show_rail {
            brush_rail(root, s, st, &cx);
        }
        if st.show_panel {
            settings_dock(root, s, st, cam, giz, &mut cx);
        }
    }

    // Whatever the docks left over is the viewport, and that is where the
    // orientation widget belongs.
    if st.show_nav {
        let viewport = root.available_rect_before_wrap();
        if let Some(a) = st.nav.show(root.ctx(), viewport, cam, &p, &m) {
            match a {
                NavAction::View(preset) => cam.set_preset(preset),
                NavAction::Align => {
                    let (preset, _) = navwidget::nearest_view(cam);
                    cam.set_preset(preset);
                }
                NavAction::Frame => actions.push(Action::FrameView),
                NavAction::ToggleProjection => {
                    cam.projection = match cam.projection {
                        Projection::Perspective => Projection::Orthographic,
                        Projection::Orthographic => Projection::Perspective,
                    }
                }
                NavAction::ToggleLock => {
                    cam.locked = !cam.locked;
                    st.say(if cam.locked {
                        "angle locked, pan and zoom still free"
                    } else {
                        "angle unlocked"
                    });
                }
                NavAction::Orbit(d) => cam.orbit(d.x, d.y),
            }
        }
    }
    // Floating controls sit over the viewport, under the radial menu.
    let viewport = root.available_rect_before_wrap();
    st.viewport = viewport;

    // Remember the last pointer position that belonged to the model. Once a
    // slider takes the pointer, egui owns it and this stops updating, which is
    // exactly what keeps the brush preview where the brush is.
    if !root.ctx().egui_wants_pointer_input() {
        if let Some(at) = root.ctx().pointer_latest_pos() {
            if viewport.contains(at) {
                st.viewport_cursor = at;
            }
        }
    }
    if !viewport.contains(st.viewport_cursor) {
        st.viewport_cursor = viewport.center();
    }
    let cursor = st.viewport_cursor;
    let wheel_open = st.wheel.open;
    let picking = st.picking_color.then_some(st.hover_color).flatten();
    for a in st.hud.show(root.ctx(), viewport, s, &p, wheel_open, picking) {
        match a {
            HudAction::OpenWheel => {
                // Summoned from a floating button, the menu always opens in the
                // middle. The button sits at the edge of the screen, and a menu
                // wrapped around it would put half its ring off the window and
                // the other half under your hand.
                st.wheel.open_at(viewport.center(), s);
            }
            HudAction::Pan(d) => cam.pan(d.x, d.y, viewport.height().max(1.0)),
            HudAction::Zoom(amount) => cam.zoom(amount),
        }
    }

    viewport_overlay(root, s, st, overlay, p);

    // While size or force is being dragged, show the brush at its new size so
    // the number is not the only thing to go on.
    //
    // Always in the middle. The pointer is on the slider while it drags, and
    // the last place it was over the model is wherever you happened to leave
    // it, which is no better a place to judge a size from than any other.
    if st.hud.is_adjusting() || st.wheel.is_adjusting() {
        brush_preview(root.ctx(), viewport.center(), s, cam, viewport, p);
    }
    let _ = cursor;

    // Last, so it covers everything else while it is up.
    if let Some(crate::wheel::WheelRequest::PickColor) = st.wheel.show(root.ctx(), viewport, s, &p) {
        st.picking_color = true;
        st.wheel.close();
        st.say("pick a colour from the model");
    }

    actions
}

/// A ghost of the brush, drawn at the anchor while its size is being changed.
///
/// Measured at the pivot rather than under the cursor: there may be no surface
/// under the cursor at all, and the pivot is where the model is.
fn brush_preview(
    ctx: &egui::Context,
    at: egui::Pos2,
    s: &Sculptor,
    cam: &Camera,
    viewport: egui::Rect,
    p: Palette,
) {
    let ppp = ctx.pixels_per_point().max(1e-3);
    let height_px = viewport.height() * ppp;
    let radius = cam.world_radius_to_pixels(cam.target(), s.brush.radius, height_px) / ppp;
    if !radius.is_finite() || radius <= 0.5 {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("brush_preview"),
    ));
    painter.circle_stroke(at, radius, Stroke::new(1.6, p.accent));
    painter.circle_stroke(
        at,
        radius * s.brush.strength.clamp(0.05, 1.0),
        Stroke::new(1.0, p.accent.gamma_multiply(0.45)),
    );
    painter.circle_filled(at, 2.0, p.accent);
}

/// A readable name for a key, for the shortcut display.
pub fn key_name(code: KeyCode) -> String {
    let raw = format!("{code:?}");
    // winit spells them `KeyA`, `Digit1`, `ShiftLeft`; trim the category.
    for prefix in ["Key", "Digit", "Numpad"] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            return if prefix == "Numpad" {
                format!("Numpad {rest}")
            } else {
                rest.to_string()
            };
        }
    }
    raw
}

/// Builds a panel on the requested side.
fn dock(id: &'static str, side: Side) -> egui::Panel {
    match side {
        Side::Left => egui::Panel::left(id),
        Side::Right => egui::Panel::right(id),
        Side::Top => egui::Panel::top(id),
        Side::Bottom => egui::Panel::bottom(id),
    }
}

// ---------------------------------------------------------------------------
// top bar
// ---------------------------------------------------------------------------

fn top_bar(
    root: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    cam: &mut Camera,
    giz: &mut Gizmo,
    cx: &mut Ctx,
) {
    let (p, m) = (cx.p, cx.m);
    let height = m.button + 14.0;
    egui::Panel::top("topbar")
        .exact_size(height)
        .frame(
            Frame::new()
                .fill(p.panel)
                .inner_margin(Margin::symmetric(m.pad as i8, 6)),
        )
        .show(root, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(
                    egui::RichText::new("sculpt")
                        .strong()
                        .color(p.accent)
                        .size(m.row * 0.52),
                );
                ui.add_space(m.gap);
                separator(ui, height, p);

                let b = m.button;
                if widgets::icon_button(ui, Icon::New, b, false, "New sphere").clicked() {
                    cx.actions.push(Action::New(Primitive::Sphere));
                }
                if widgets::icon_button(ui, Icon::Open, b, false, "Open a mesh or scene").clicked() {
                    cx.actions.push(Action::Import);
                }
                if widgets::icon_button(ui, Icon::Save, b, false, "Export the active mesh").clicked()
                {
                    cx.actions.push(Action::Export);
                }
                separator(ui, height, p);

                let can_undo = s.history.can_undo();
                let can_redo = s.history.can_redo();
                ui.add_enabled_ui(can_undo, |ui| {
                    if widgets::icon_button(ui, Icon::Undo, b, false, "Undo   Ctrl+Z").clicked() {
                        cx.actions.push(Action::Undo);
                    }
                });
                ui.add_enabled_ui(can_redo, |ui| {
                    if widgets::icon_button(ui, Icon::Redo, b, false, "Redo   Ctrl+Shift+Z")
                        .clicked()
                    {
                        cx.actions.push(Action::Redo);
                    }
                });
                separator(ui, height, p);

                // Symmetry: one toggle plus the plane it works on.
                if widgets::icon_button(ui, Icon::Symmetry, b, s.symmetry, "Symmetry   X").clicked()
                {
                    s.symmetry = !s.symmetry;
                }
                let current = s.symmetry_axis.index();
                ui.scope(|ui| {
                    ui.set_width(b * 2.4);
                    if let Some(i) = widgets::segmented(ui, &["X", "Y", "Z"], current, b * 0.72) {
                        s.symmetry_axis = Axis::ALL[i];
                        s.symmetry = true;
                    }
                });
                separator(ui, height, p);

                if widgets::icon_button(ui, Icon::Grid, b, st.settings.grid, "Ground grid   G")
                    .clicked()
                {
                    st.settings.grid = !st.settings.grid;
                }
                ui.add_enabled_ui(st.wireframe_available, |ui| {
                    if widgets::icon_button(
                        ui,
                        Icon::Wireframe,
                        b,
                        st.settings.wireframe,
                        "Wireframe   W",
                    )
                    .clicked()
                    {
                        st.settings.wireframe = !st.settings.wireframe;
                    }
                });
                if widgets::icon_button(ui, Icon::FrameView, b, false, "Frame the model   F")
                    .clicked()
                {
                    cx.actions.push(Action::FrameView);
                }
                // One button cycles the gizmo through its three modes and off.
                let (gizmo_icon, gizmo_tip) = match giz.mode {
                    GizmoMode::Off => (Icon::Move, "Transform gizmo: off   T"),
                    GizmoMode::Move => (Icon::Move, "Gizmo: move   T"),
                    GizmoMode::Rotate => (Icon::Twist, "Gizmo: rotate   T"),
                    GizmoMode::Scale => (Icon::Scale, "Gizmo: scale   T"),
                };
                if widgets::icon_button(ui, gizmo_icon, b, giz.mode != GizmoMode::Off, gizmo_tip)
                    .clicked()
                {
                    giz.mode = next_gizmo_mode(giz.mode);
                }
                if widgets::icon_button(
                    ui,
                    Icon::Camera,
                    b,
                    cam.projection == Projection::Orthographic,
                    "Orthographic view   Numpad 5",
                )
                .clicked()
                {
                    cam.projection = match cam.projection {
                        Projection::Perspective => Projection::Orthographic,
                        Projection::Orthographic => Projection::Perspective,
                    };
                }

                // Right-aligned cluster.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(
                        ui,
                        Icon::Menu,
                        b,
                        st.show_panel,
                        "Show or hide the dock   Tab",
                    )
                    .clicked()
                    {
                        st.show_panel = !st.show_panel;
                    }
                    if widgets::icon_button(ui, Icon::Settings, b, st.theme.touch, "Touch layout")
                        .clicked()
                    {
                        st.theme.touch = !st.theme.touch;
                    }
                    if widgets::icon_button(ui, Icon::Pin, b, st.show_help, "Shortcuts   H")
                        .clicked()
                    {
                        st.show_help = !st.show_help;
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "CPU {:>4.1} ms",
                            st.frame_ms,
                        ))
                            .small()
                            .monospace()
                            .color(if st.frame_ms > 40.0 { p.warn } else { p.faint }),
                    );
                });
            });
        });
}

/// Human-readable byte count for the statistics readout.
fn format_bytes(n: u64) -> String {
    match n {
        0 => "none".into(),
        n if n < 1024 => format!("{n} B"),
        n if n < 1024 * 1024 => format!("{:.0} kB", n as f64 / 1024.0),
        n => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
    }
}

/// Off, move, rotate, scale, and back to off.
pub fn next_gizmo_mode(mode: GizmoMode) -> GizmoMode {
    match mode {
        GizmoMode::Off => GizmoMode::Move,
        GizmoMode::Move => GizmoMode::Rotate,
        GizmoMode::Rotate => GizmoMode::Scale,
        GizmoMode::Scale => GizmoMode::Off,
    }
}

fn separator(ui: &mut egui::Ui, height: f32, p: Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, height * 0.45), Sense::hover());
    ui.painter().line_segment(
        [
            egui::pos2(rect.center().x, rect.top()),
            egui::pos2(rect.center().x, rect.bottom()),
        ],
        Stroke::new(1.0, p.line),
    );
}

// ---------------------------------------------------------------------------
// brush rail
// ---------------------------------------------------------------------------

fn brush_rail(root: &mut egui::Ui, s: &mut Sculptor, st: &UiState, cx: &Ctx) {
    let (p, m) = (cx.p, cx.m);
    let side = st.theme.rail_side;
    let horizontal = side.is_horizontal();
    // A horizontal rail has no room for labels, so it uses square chips.
    let chip = m.tool * 0.82;
    let thickness = if horizontal {
        chip + m.pad * 2.0
    } else {
        m.tool + m.pad * 2.0
    };

    dock("rail", side)
        .exact_size(thickness)
        .frame(
            Frame::new()
                .fill(p.panel)
                .inner_margin(Margin::same(m.pad as i8)),
        )
        .show(root, |ui| {
            let scroll = if horizontal {
                egui::ScrollArea::horizontal()
            } else {
                egui::ScrollArea::vertical()
            };
            // No bar on the rail: it is a strip of tools, and a scroll bar down
            // the side of it is just clutter. Dragging and the wheel still
            // scroll it when the tools do not all fit.
            scroll
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                let build = |ui: &mut egui::Ui, s: &mut Sculptor| {
                    ui.spacing_mut().item_spacing = Vec2::splat(4.0);
                    for (i, kind) in BrushKind::ALL.iter().enumerate() {
                        let selected = s.brush.kind == *kind;
                        let tip = format!(
                            "{}\n{}\n\nShortcut: {}",
                            kind.label(),
                            kind.hint(),
                            shortcut_for(i)
                        );
                        let clicked = if horizontal {
                            widgets::tool_chip(ui, Icon::of_brush(*kind), chip, selected, &tip)
                                .clicked()
                        } else {
                            widgets::tool_tile(
                                ui,
                                Icon::of_brush(*kind),
                                kind.label(),
                                m.tool,
                                selected,
                                &tip,
                            )
                            .clicked()
                        };
                        if clicked {
                            s.set_brush_kind(*kind);
                        }
                    }
                };
                if horizontal {
                    ui.horizontal_centered(|ui| build(ui, s));
                } else {
                    build(ui, s);
                }
            });
        });
}

/// Keyboard shortcut shown for the nth tool.
pub fn shortcut_for(index: usize) -> String {
    match index {
        0..=8 => format!("{}", index + 1),
        9 => "0".into(),
        10..=15 => format!("Shift+{}", index - 9),
        _ => "-".into(),
    }
}

// ---------------------------------------------------------------------------
// settings dock
// ---------------------------------------------------------------------------

fn settings_dock(
    root: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    cam: &mut Camera,
    giz: &mut Gizmo,
    cx: &mut Ctx,
) {
    let (p, m) = (cx.p, cx.m);
    // Fixed width, set by the slider in the UI tab rather than by dragging the
    // edge.
    //
    // A resizable panel remembers the size its content reported last frame, so
    // one widget overflowing by a few points makes the panel wider, and a wider
    // panel gives that widget more room to overflow into. That loop is what
    // walked the dock out to its maximum in the tabs carrying the most
    // controls. Taking the width from our own state breaks it outright.
    dock("panel", st.theme.panel_side)
        .exact_size(m.panel_width)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(p.panel)
                .inner_margin(Margin::same(m.pad as i8)),
        )
        .show(root, |ui| {
            // Explanatory text has to wrap. Left to itself a label asks for its
            // full unbroken width, and a resizable dock grows to grant it,
            // which is how one long sentence swallows the viewport.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);

            // Only the docked tabs are offered here. A tab that has been torn
            // off is already on screen, and listing it twice would leave the
            // dock showing a copy of a window sitting next to it.
            let docked: Vec<Tab> = Tab::ALL
                .iter()
                .copied()
                .filter(|t| !st.floating.contains(t))
                .collect();
            if docked.is_empty() {
                ui.label(
                    egui::RichText::new("Every panel is floating.")
                        .small()
                        .color(p.dim),
                );
            } else {
                if !docked.contains(&st.tab) {
                    st.tab = docked[0];
                }
                let labels: Vec<&str> = docked.iter().map(|t| t.label()).collect();
                let current = docked.iter().position(|t| *t == st.tab).unwrap_or(0);
                if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
                    st.tab = docked[i];
                }
                ui.add_space(4.0);
                if widgets::wide_button(ui, Icon::Pin, "Float this panel", m.row * 0.85, false)
                    .clicked()
                {
                    let tab = st.tab;
                    st.floating.push(tab);
                }
                ui.add_space(4.0);

                let inner_width = ui.available_width();
                let tab = st.tab;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(inner_width);
                        tab_body(ui, tab, s, st, cam, giz, cx);
                    });
            }
        });

    floating_panels(root, s, st, cam, giz, cx);
}

/// Draws whatever a tab holds, wherever it happens to be living.
fn tab_body(
    ui: &mut egui::Ui,
    tab: Tab,
    s: &mut Sculptor,
    st: &mut UiState,
    cam: &mut Camera,
    giz: &mut Gizmo,
    cx: &mut Ctx,
) {
    match tab {
        Tab::Brush => brush_tab(ui, s, st, cx),
        Tab::Colour => colour_tab(ui, s, st, cx),
        Tab::Materials => materials_tab(ui, s, st, cx),
        Tab::Model => model_tab(ui, s, st, cx),
        Tab::Scene => scene_tab(ui, s, st, giz, cx),
        Tab::View => view_tab(ui, st, cam, cx),
        Tab::Interface => interface_tab(ui, st, cx),
    }
}

/// The tabs that have been torn out of the dock, each in its own window.
///
/// Windows rather than extra docks: a floating panel is for the one control set
/// you are working through right now, and it should be able to sit over the
/// model, next to what it is changing, instead of stealing another edge of the
/// screen.
fn floating_panels(
    root: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    cam: &mut Camera,
    giz: &mut Gizmo,
    cx: &mut Ctx,
) {
    let (p, m) = (cx.p, cx.m);
    let open: Vec<Tab> = st.floating.clone();
    for (i, tab) in open.into_iter().enumerate() {
        let mut still_open = true;
        egui::Window::new(tab.label())
            .id(egui::Id::new(("floating", tab)))
            .open(&mut still_open)
            .default_pos(egui::pos2(
                st.viewport.left() + 40.0 + i as f32 * 26.0,
                st.viewport.top() + 40.0 + i as f32 * 26.0,
            ))
            .default_width(m.panel_width)
            // A window sizes itself to what it holds, and what it holds asks
            // for the width it is given: sliders here are full-width rails by
            // design. Left alone the two chase each other to the edge of the
            // screen on the first frame, and the window never comes back
            // however it is dragged. The ceiling breaks the circle and still
            // leaves room to make it as wide as anyone would want.
            .max_width((st.viewport.width() * 0.45).max(m.panel_width * 1.5))
            .resizable(true)
            .frame(
                Frame::new()
                    .fill(p.panel)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(Margin::same(m.pad as i8)),
            )
            .show(root.ctx(), |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                let inner_width = ui.available_width();
                egui::ScrollArea::vertical()
                    // Allowed to shrink. Told to take everything available, it
                    // reports that as the width it needs, and the window takes
                    // that for a minimum: it could be dragged wider and never
                    // narrower, snapping back on release. The content is pinned
                    // to the window below instead, which fills it just the same
                    // and follows it back in.
                    .auto_shrink([true, true])
                    .max_height(st.viewport.height() * 0.75)
                    .show(ui, |ui| {
                        ui.set_width(inner_width);
                        tab_body(ui, tab, s, st, cam, giz, cx);
                    });
            });
        if !still_open {
            // Closing a floating panel puts it back where it came from rather
            // than hiding it: there is no other way to get it back.
            st.floating.retain(|t| *t != tab);
            st.tab = tab;
        }
    }
}

fn brush_tab(ui: &mut egui::Ui, s: &mut Sculptor, st: &mut UiState, cx: &mut Ctx) {
    let (p, m) = (cx.p, cx.m);
    let kind = s.brush.kind;
    ui.label(
        egui::RichText::new(kind.label())
            .strong()
            .size(m.row * 0.5)
            .color(p.text),
    );
    ui.label(egui::RichText::new(kind.hint()).small().color(p.dim));
    ui.add_space(6.0);

    BigSlider::new(&mut s.brush.radius, 0.005..=1.5, "Radius")
        .logarithmic(true)
        .decimals(3)
        .height(m.row)
        .show(ui);
    BigSlider::new(&mut s.brush.strength, 0.0..=1.0, "Strength")
        .height(m.row)
        .show(ui);

    if kind.has_negative() {
        widgets::toggle(ui, &mut s.brush.negative, "Invert   hold Ctrl", m.row);
    }

    widgets::section_title(ui, "FALLOFF");
    let labels: Vec<&str> = Falloff::ALL.iter().map(|f| f.label()).collect();
    let current = Falloff::ALL
        .iter()
        .position(|f| *f == s.brush.falloff)
        .unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
        s.brush.falloff = Falloff::ALL[i];
    }
    falloff_preview(ui, s.brush.falloff, &m, p);

    alpha_picker(ui, st, s, cx, &m, p);

    widgets::section_title(ui, "BEHAVIOUR");
    BigSlider::new(&mut s.brush.spacing, 0.02..=1.0, "Dab spacing")
        .decimals(2).height(m.row).show(ui);
    widgets::toggle(ui, &mut s.brush.culling, "Front faces only", m.row);
    widgets::toggle(ui, &mut s.brush.lock_plane, "Lock the stroke plane", m.row);
    BigSlider::new(&mut s.brush.auto_smooth, 0.0..=1.0, "Auto smooth")
        .height(m.row)
        .show(ui);
    BigSlider::new(&mut st.brush_reach, 0.0..=40.0, "Reach past the edge")
        .decimals(0)
        .suffix(" px")
        .height(m.row)
        .show(ui);
    ui.label(
        egui::RichText::new(
            "How far off the silhouette a stroke will still find the surface. Keep it small: the wider it is, the harder it is to miss the model on purpose.",
        )
        .small()
        .color(p.dim),
    );

    widgets::section_title(ui, "PRESSURE");
    widgets::toggle(ui, &mut s.brush.pressure_radius, "Pressure drives radius", m.row);
    widgets::toggle(
        ui,
        &mut s.brush.pressure_strength,
        "Pressure drives strength",
        m.row,
    );

    if kind.paints() {
        widgets::section_title(ui, "COLOUR");
        let mut rgb = [
            s.brush.paint_color.x,
            s.brush.paint_color.y,
            s.brush.paint_color.z,
        ];
        if widgets::color_row(ui, "Colour", &mut rgb, m.row).changed() {
            s.brush.paint_color = glam::Vec3::from_array(rgb);
        }
        if widgets::wide_button(
            ui,
            Icon::Palette,
            "Pick a colour from the model",
            m.row,
            st.picking_color,
        )
        .clicked()
        {
            cx.actions.push(Action::PickColorMode);
        }

        if matches!(kind, BrushKind::Paint | BrushKind::Fill) {
            let labels: Vec<&str> = BlendMode::ALL.iter().map(|b| b.label()).collect();
            let current = BlendMode::ALL
                .iter()
                .position(|b| *b == s.brush.blend)
                .unwrap_or(0);
            if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
                s.brush.blend = BlendMode::ALL[i];
            }
        }

        if kind == BrushKind::Paint {
            BigSlider::new(&mut s.brush.flow, 0.02..=1.0, "Flow")
                .height(m.row)
                .show(ui);
            widgets::toggle(ui, &mut s.brush.paint_albedo, "Paint colour", m.row);
            widgets::toggle(ui, &mut s.brush.paint_material, "Paint material", m.row);
            let material = s.brush.paint_material;
            ui.add_enabled_ui(material, |ui| {
                BigSlider::new(&mut s.brush.paint_rough, 0.02..=1.0, "Roughness")
                    .height(m.row)
                    .show(ui);
                BigSlider::new(&mut s.brush.paint_metal, 0.0..=1.0, "Metalness")
                    .height(m.row)
                    .show(ui);
            });
        }

        if kind == BrushKind::Smudge {
            BigSlider::new(&mut s.brush.smudge_pickup, 0.0..=1.0, "Pick up")
                .height(m.row)
                .show(ui);
            ui.label(
                egui::RichText::new("Low pick up drags one colour a long way.")
                    .small()
                    .color(p.dim),
            );
        }

        if kind == BrushKind::Fill {
            widgets::section_title(ui, "SPREAD");
            let labels: Vec<&str> = FillScope::ALL.iter().map(|f| f.label()).collect();
            let current = FillScope::ALL
                .iter()
                .position(|f| *f == s.brush.fill_scope)
                .unwrap_or(0);
            if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
                s.brush.fill_scope = FillScope::ALL[i];
            }
            let region = s.brush.fill_scope == FillScope::Region;
            ui.add_enabled_ui(region, |ui| {
                BigSlider::new(&mut s.brush.fill_angle, 1.0..=180.0, "Stop at")
                    .decimals(0)
                    .suffix("°")
                    .height(m.row)
                    .show(ui);
            });
            ui.label(
                egui::RichText::new("A fill spills across the surface until it meets an edge sharper than that, or a masked vertex.")
                    .small()
                    .color(p.dim),
            );
        }

        if st.settings.shading != Shading::Unlit {
            ui.add_space(4.0);
            if widgets::wide_button(ui, Icon::Palette, "Switch to unlit", m.row, false).clicked() {
                st.settings.shading = Shading::Unlit;
            }
        }
    }

    if kind == BrushKind::Mask {
        widgets::section_title(ui, "MASK");
        mask_controls(ui, st, cx);
    }

    saved_brushes(ui, s, st, cx);

    ui.add_space(8.0);
    if widgets::wide_button(ui, Icon::Reset, "Reset every tool", m.row, false).clicked() {
        cx.actions.push(Action::ResetBrushes);
    }
}

/// Brushes the artist built and named, kept apart from the per-tool defaults.
///
/// Switching tools already remembers what each one was set to. This is the
/// other thing people mean by a custom brush: a particular tool with a
/// particular size, falloff, alpha and colour, worth keeping and coming back
/// to, and worth carrying between sessions.
fn saved_brushes(ui: &mut egui::Ui, s: &mut Sculptor, st: &mut UiState, cx: &mut Ctx) {
    let (p, m) = (cx.p, cx.m);
    widgets::section_title(ui, "MY BRUSHES");

    let mut apply = None;
    let mut remove = None;
    let mut duplicate = None;
    let mut reorder = None;
    let count = st.brushes.len();
    for (i, nb) in st.brushes.iter_mut().enumerate() {
        ui.push_id(i, |ui| ui.horizontal(|ui| {
            let live = s.brush.kind == nb.brush.kind
                && (s.brush.radius - nb.brush.radius).abs() < 1e-4
                && (s.brush.strength - nb.brush.strength).abs() < 1e-4
                && s.brush.falloff == nb.brush.falloff
                && s.brush.alpha == nb.brush.alpha;
            let icon = nb.icon.as_deref().and_then(BrushKind::from_label).unwrap_or(nb.brush.kind);
            let texture = nb.icon.as_ref().and_then(|key| {
                if !st.brush_icons.contains_key(key) {
                    let hex = key.strip_prefix("rgba64:")?;
                    if hex.len() != 64 * 64 * 8 || !hex.is_ascii() { return None; }
                    let bytes: Option<Vec<u8>> = (0..hex.len()).step_by(2)
                        .map(|n| u8::from_str_radix(&hex[n..n+2], 16).ok()).collect();
                    let image = egui::ColorImage::from_rgba_unmultiplied([64, 64], &bytes?);
                    st.brush_icons.insert(key.clone(), ui.ctx().load_texture(
                        format!("custom brush {}", st.brush_icons.len()), image, egui::TextureOptions::LINEAR));
                }
                st.brush_icons.get(key).map(|texture| texture.id())
            });
            let response = if let Some(texture) = texture {
                ui.add(egui::Button::image_and_text(
                    egui::Image::new((texture, egui::vec2(m.row - 6.0, m.row - 6.0))), &nb.name)
                    .selected(live).min_size(egui::vec2(80.0, m.row)))
            } else {
                widgets::wide_button(ui, Icon::of_brush(icon), &nb.name, m.row, live)
            };
            if response.clicked() { apply = Some(i); }
            let mut edit = |ui: &mut egui::Ui| {
                ui.label("Brush name");
                ui.text_edit_singleline(&mut nb.name);
                if ui.button("Replace settings with current brush").clicked() {
                    nb.brush = s.brush;
                    ui.close();
                }
                if ui.button("Duplicate").clicked() { duplicate = Some(i); ui.close(); }
                ui.horizontal(|ui| {
                    if ui.add_enabled(i > 0, egui::Button::new("Move up")).clicked() {
                        reorder = Some((i, i - 1)); ui.close();
                    }
                    if ui.add_enabled(i + 1 < count, egui::Button::new("Move down")).clicked() {
                        reorder = Some((i, i + 1)); ui.close();
                    }
                });
                ui.separator();
                ui.label("Icon");
                ui.horizontal_wrapped(|ui| {
                    for kind in BrushKind::ALL {
                        if widgets::icon_button(ui, Icon::of_brush(kind), m.row,
                            nb.icon.as_deref() == Some(kind.label()), kind.label()).clicked() {
                            nb.icon = Some(kind.label().into());
                        }
                    }
                });
                if ui.button("Import image as icon...").clicked() {
                    cx.actions.push(Action::LoadBrushIcon(i)); ui.close();
                }
                if ui.button("Restore tool icon").clicked() { nb.icon = None; }
                ui.separator();
                if ui.button("Delete brush").clicked() { remove = Some(i); ui.close(); }
            };
            // Visible menu also works on touch devices without a right button.
            ui.menu_button("...", &mut edit);
            response.context_menu(&mut edit);
        }));
    }
    if let Some(i) = duplicate {
        let mut brush = st.brushes[i].clone();
        brush.name.push_str(" copy");
        st.brushes.insert(i + 1, brush);
    }
    if let Some((from, to)) = reorder { st.brushes.swap(from, to); }
    if st.brushes.is_empty() {
        ui.label(
            egui::RichText::new("Nothing kept yet. Set a tool up the way you like it, then keep it.")
                .small()
                .color(p.dim),
        );
    }
    if let Some(i) = apply {
        let nb = st.brushes[i].clone();
        s.set_brush_kind(nb.brush.kind);
        s.brush = nb.brush;
        st.say(format!("{} ready", nb.name));
    }
    if let Some(i) = remove {
        let gone = st.brushes.remove(i);
        st.say(format!("{} dropped", gone.name));
    }

    ui.horizontal(|ui| {
        let width = ui.available_width();
        ui.add_sized(
            [width * 0.6, m.row],
            egui::TextEdit::singleline(&mut st.brush_name).hint_text("Name"),
        );
        if widgets::wide_button(ui, Icon::Plus, "Keep", m.row, false).clicked() {
            let name = if st.brush_name.trim().is_empty() {
                format!("{} {}", s.brush.kind.label(), st.brushes.len() + 1)
            } else {
                st.brush_name.trim().to_string()
            };
            st.brushes.push(io::NamedBrush { name, brush: s.brush, icon: None });
            st.brush_name.clear();
        }
    });
    ui.horizontal(|ui| {
        if widgets::wide_button(ui, Icon::Save, "Save set", m.row, false).clicked() {
            cx.actions.push(Action::SaveBrushes);
        }
        if widgets::wide_button(ui, Icon::Open, "Load set", m.row, false).clicked() {
            cx.actions.push(Action::LoadBrushes);
        }
    });
}

/// The colour panel: a square, a hue strip, harmonies and a palette.
///
/// The radial menu already carries a hue ring for the colour you need in the
/// middle of a stroke. This is the other half of the job: sitting down and
/// choosing a scheme. It is a tab like any other, so it can be torn off and
/// parked beside the model while a painting pass goes on.
fn materials_tab(ui: &mut egui::Ui, s: &mut Sculptor, st: &mut UiState, cx: &mut Ctx) {
    let (p, m) = (cx.p, cx.m);
    widgets::section_title(ui, "MATERIAL LIBRARY");
    ui.label("Choose a material to paint colour, roughness and metalness together. Lit shading shows their response to light.");
    let mut selected = None;
    let mut remove = None;
    for (index, material) in st.materials.iter_mut().enumerate() {
        ui.push_id(index, |ui| ui.horizontal(|ui| {
            if colour_chip(ui, glam::Vec3::from_array(material.color), m.row, p).clicked() {
                selected = Some(index);
            }
            if ui.button(&material.name).clicked() { selected = Some(index); }
            ui.menu_button("...", |ui| {
                ui.text_edit_singleline(&mut material.name);
                ui.color_edit_button_rgb(&mut material.color);
                ui.add(egui::Slider::new(&mut material.roughness, 0.02..=1.0).text("Roughness"));
                ui.add(egui::Slider::new(&mut material.metalness, 0.0..=1.0).text("Metalness"));
                if ui.button("Delete").clicked() { remove = Some(index); ui.close(); }
            });
        }));
    }
    if let Some(index) = selected {
        let material = &st.materials[index];
        s.set_brush_kind(BrushKind::Paint);
        s.brush.paint_color = glam::Vec3::from_array(material.color);
        s.brush.paint_rough = material.roughness;
        s.brush.paint_metal = material.metalness;
        s.brush.paint_albedo = true;
        s.brush.paint_material = true;
        st.settings.shading = Shading::Pbr;
        st.settings.vertex_color = true;
    }
    if let Some(index) = remove { st.materials.remove(index); }
    ui.separator();
    ui.add(egui::TextEdit::singleline(&mut st.material_name).hint_text("Material name"));
    if widgets::wide_button(ui, Icon::Plus, "Keep current paint material", m.row, false).clicked() {
        st.materials.push(crate::materials::Material {
            name: if st.material_name.trim().is_empty() { format!("Material {}", st.materials.len() + 1) } else { st.material_name.trim().into() },
            color: s.brush.paint_color.to_array(), roughness: s.brush.paint_rough, metalness: s.brush.paint_metal,
        });
        st.material_name.clear();
    }
    ui.separator();
    widgets::section_title(ui, "MESH ASSETS");
    if widgets::wide_button(ui, Icon::Open, "Add mesh to scene", m.row, false).clicked() {
        cx.actions.push(Action::ImportObject);
    }
    ui.label("OBJ, PLY and STL. Existing objects stay in the scene; use Scene to select, rename, transform or merge them.");
    widgets::section_title(ui, "STAMP TEXTURES");
    alpha_picker(ui, st, s, cx, &m, p);
}

fn colour_tab(ui: &mut egui::Ui, s: &mut Sculptor, st: &mut UiState, cx: &mut Ctx) {
    let (p, m) = (cx.p, cx.m);
    let colour = s.brush.paint_color;
    let (mut hue, mut sat, mut val) = wheel::rgb_to_hsv(colour.x, colour.y, colour.z);
    let grey = st.wheel.grayscale;
    // Only a deliberate change is written back. Reading the colour as hue,
    // saturation and value and writing it straight out again every frame would
    // grind it down through the rounding, and a grey has no hue to keep at all.
    let mut touched = false;

    // Saturation and value.
    let side = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        egui::Vec2::new(side, side * 0.62),
        egui::Sense::click_and_drag(),
    );
    if response.dragged() || response.clicked() {
        if let Some(at) = ui.ctx().pointer_interact_pos() {
            sat = ((at.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            val = ((rect.bottom() - at.y) / rect.height()).clamp(0.0, 1.0);
            touched = true;
        }
    }
    let (hr, hg, hb) = if grey { (1.0, 1.0, 1.0) } else { wheel::hsv_to_rgb(hue, 1.0, 1.0) };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_bottom(), egui::Color32::BLACK);
    mesh.colored_vertex(rect.right_bottom(), egui::Color32::BLACK);
    mesh.colored_vertex(rect.left_top(), egui::Color32::WHITE);
    mesh.colored_vertex(
        rect.right_top(),
        egui::Color32::from_rgb((hr * 255.0) as u8, (hg * 255.0) as u8, (hb * 255.0) as u8),
    );
    mesh.add_triangle(0, 1, 3);
    mesh.add_triangle(0, 3, 2);
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(4),
        Stroke::new(1.0, p.line),
        egui::StrokeKind::Outside,
    );
    ui.painter().circle_stroke(
        egui::Pos2::new(
            rect.left() + sat * rect.width(),
            rect.bottom() - val * rect.height(),
        ),
        6.0,
        Stroke::new(1.8, if val > 0.55 { egui::Color32::BLACK } else { egui::Color32::WHITE }),
    );

    // Hue, as a strip under the square.
    let (strip, hue_response) = ui.allocate_exact_size(
        egui::Vec2::new(side, m.row * 0.7),
        egui::Sense::click_and_drag(),
    );
    if hue_response.dragged() || hue_response.clicked() {
        if let Some(at) = ui.ctx().pointer_interact_pos() {
            hue = ((at.x - strip.left()) / strip.width()).clamp(0.0, 1.0);
            touched = true;
        }
    }
    let steps = 48;
    for i in 0..steps {
        let t0 = i as f32 / steps as f32;
        let t1 = (i + 1) as f32 / steps as f32;
        let (r, g, b) = if grey {
            (t0, t0, t0)
        } else {
            wheel::hsv_to_rgb(t0, 1.0, 1.0)
        };
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::Pos2::new(strip.left() + t0 * strip.width(), strip.top()),
                egui::Pos2::new(strip.left() + t1 * strip.width(), strip.bottom()),
            ),
            0.0,
            egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8),
        );
    }
    let marker = strip.left() + if grey { val } else { hue } * strip.width();
    ui.painter().line_segment(
        [egui::Pos2::new(marker, strip.top()), egui::Pos2::new(marker, strip.bottom())],
        Stroke::new(2.0, p.text),
    );

    let mut next = if grey {
        let v = if hue_response.dragged() || hue_response.clicked() {
            hue
        } else {
            val
        };
        glam::Vec3::splat(v)
    } else {
        let (r, g, b) = wheel::hsv_to_rgb(hue, sat, val);
        glam::Vec3::new(r, g, b)
    };

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let swatch = egui::Rect::from_min_size(
            ui.cursor().min,
            egui::Vec2::new(m.row * 1.6, m.row),
        );
        ui.allocate_rect(swatch, egui::Sense::hover());
        ui.painter().rect(
            swatch,
            egui::CornerRadius::same(5),
            egui::Color32::from_rgb(
                (next.x * 255.0) as u8,
                (next.y * 255.0) as u8,
                (next.z * 255.0) as u8,
            ),
            Stroke::new(1.0, p.line),
            egui::StrokeKind::Inside,
        );
        ui.label(
            egui::RichText::new(format!(
                "#{:02X}{:02X}{:02X}",
                (next.x.clamp(0.0, 1.0) * 255.0) as u8,
                (next.y.clamp(0.0, 1.0) * 255.0) as u8,
                (next.z.clamp(0.0, 1.0) * 255.0) as u8
            ))
            .monospace()
            .color(p.text),
        );
    });

    widgets::toggle(ui, &mut st.wheel.grayscale, "Greys only", m.row);
    if widgets::wide_button(ui, Icon::Palette, "Pick off the model", m.row, st.picking_color)
        .clicked()
    {
        cx.actions.push(Action::PickColorMode);
    }
    // While the eyedropper is armed, what it is pointing at is worth more than
    // what it took last time.
    if st.picking_color {
        ui.horizontal(|ui| {
            match st.hover_color {
                Some(c) => {
                    colour_chip(ui, c, m.row, p);
                    ui.label(
                        egui::RichText::new(format!(
                            "#{:02X}{:02X}{:02X}",
                            (c.x.clamp(0.0, 1.0) * 255.0) as u8,
                            (c.y.clamp(0.0, 1.0) * 255.0) as u8,
                            (c.z.clamp(0.0, 1.0) * 255.0) as u8
                        ))
                        .monospace()
                        .color(p.accent),
                    );
                }
                None => {
                    ui.label(
                        egui::RichText::new("Move over the model.")
                            .small()
                            .color(p.dim),
                    );
                }
            }
        });
    }

    // Harmonies.
    widgets::section_title(ui, "HARMONY");
    let labels: Vec<&str> = Harmony::ALL.iter().map(|h| h.label()).collect();
    let current = Harmony::ALL.iter().position(|h| *h == st.harmony).unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
        st.harmony = Harmony::ALL[i];
    }
    let mates = st.harmony.mates(hue, sat, val);
    if !mates.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for c in &mates {
                if colour_chip(ui, *c, m.row, p).clicked() {
                    next = *c;
                    touched = true;
                }
            }
        });
        ui.label(
            egui::RichText::new(st.harmony.hint())
                .small()
                .color(p.dim),
        );
    }

    // Palette, shared with the radial menu so a colour saved in one is in both.
    widgets::section_title(ui, "PALETTE");
    let mut remove = None;
    let saved: Vec<[f32; 3]> = st.wheel.swatches.clone();
    ui.horizontal_wrapped(|ui| {
        for (i, c) in saved.iter().enumerate() {
            let chip = colour_chip(ui, glam::Vec3::from_array(*c), m.row, p);
            if chip.clicked() {
                next = glam::Vec3::from_array(*c);
                touched = true;
            }
            if chip.secondary_clicked() {
                remove = Some(i);
            }
        }
    });
    if let Some(i) = remove {
        st.wheel.swatches.remove(i);
    }
    ui.horizontal(|ui| {
        if widgets::wide_button(ui, Icon::Plus, "Keep this colour", m.row, false).clicked() {
            st.wheel.swatches.push([next.x, next.y, next.z]);
            if st.wheel.swatches.len() > wheel::SWATCHES {
                st.wheel.swatches.remove(0);
            }
        }
    });
    ui.label(
        egui::RichText::new("Right click a swatch to drop it.")
            .small()
            .color(p.dim),
    );

    if touched {
        s.brush.paint_color = next;
    }
}

/// One clickable colour square.
fn colour_chip(ui: &mut egui::Ui, c: glam::Vec3, size: f32, p: Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::click());
    let hovered = response.hovered();
    ui.painter().rect(
        rect,
        egui::CornerRadius::same(5),
        egui::Color32::from_rgb(
            (c.x.clamp(0.0, 1.0) * 255.0) as u8,
            (c.y.clamp(0.0, 1.0) * 255.0) as u8,
            (c.z.clamp(0.0, 1.0) * 255.0) as u8,
        ),
        Stroke::new(if hovered { 2.0 } else { 1.0 }, if hovered { p.accent } else { p.line }),
        egui::StrokeKind::Inside,
    );
    response
}

/// The alphas the brush can stamp through, as a grid of thumbnails.
///
/// A falloff can only ever make a circle. An alpha is what turns one brush into
/// cracks, scales or a photographed grain, so it belongs right under the
/// falloff rather than buried in a menu.
fn alpha_picker(
    ui: &mut egui::Ui,
    st: &mut UiState,
    s: &mut Sculptor,
    cx: &mut Ctx,
    m: &Metrics,
    p: Palette,
) {
    widgets::section_title(ui, "ALPHA");
    st.sync_alpha_thumbs(ui.ctx(), s);

    let cell = (m.button * 1.4).max(34.0);
    let spacing = 4.0;
    let per_row = ((ui.available_width() + spacing) / (cell + spacing)).floor().max(1.0) as usize;

    let mut chosen: Option<Option<u32>> = None;
    let mut index = 0usize;
    // The first cell clears the alpha, so getting back to a plain round brush
    // is one press and never a hunt through the grid.
    let slots = st.alpha_thumbs.len() + 1;
    while index < slots {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = spacing;
            for _ in 0..per_row {
                if index >= slots {
                    break;
                }
                let selected = if index == 0 {
                    s.brush.alpha.is_none()
                } else {
                    s.brush.alpha == Some(index as u32 - 1)
                };
                let (rect, response) =
                    ui.allocate_exact_size(egui::Vec2::splat(cell), egui::Sense::click());
                let hovered = response.hovered();
                ui.painter().rect(
                    rect,
                    egui::CornerRadius::same(6),
                    p.raised,
                    Stroke::new(
                        if selected { 2.0 } else { 1.0 },
                        if selected {
                            p.accent
                        } else if hovered {
                            p.text
                        } else {
                            p.line
                        },
                    ),
                    egui::StrokeKind::Inside,
                );
                let inner = rect.shrink(4.0);
                if index == 0 {
                    crate::icons::paint(ui.painter(), inner, Icon::Draw, if selected { p.accent } else { p.dim });
                } else if let Some(tex) = st.alpha_thumbs.get(index - 1) {
                    egui::Image::new((tex.id(), inner.size()))
                        .tint(if selected { p.accent } else { p.text })
                        .paint_at(ui, inner);
                }
                if response.clicked() {
                    chosen = Some(if index == 0 { None } else { Some(index as u32 - 1) });
                }
                if hovered && index > 0 {
                    if let Some(a) = s.alphas.get(index - 1) {
                        response.on_hover_text(a.name.clone());
                    }
                }
                index += 1;
            }
        });
    }
    if let Some(choice) = chosen {
        s.brush.alpha = choice;
    }

    if s.brush.alpha.is_some() {
        let mut degrees = s.brush.alpha_angle.to_degrees();
        if BigSlider::new(&mut degrees, -180.0..=180.0, "Turn")
            .decimals(0)
            .suffix("°")
            .height(m.row)
            .show(ui)
            .changed()
        {
            s.brush.alpha_angle = degrees.to_radians();
        }
        widgets::toggle(ui, &mut s.brush.alpha_follow, "Follow the stroke", m.row);
        ui.label(
            egui::RichText::new(
                "Without this the stamp keeps the angle it has on screen, which is what you want for scales or a grain. With it, it turns to face the way the stroke is going, which is what you want for a scratch.",
            )
            .small()
            .color(p.dim),
        );
    }
    if widgets::wide_button(ui, Icon::Open, "Load an image", m.row, false).clicked() {
        cx.actions.push(Action::LoadAlpha);
    }
}

/// Little curve showing what the selected falloff does.
fn falloff_preview(ui: &mut egui::Ui, falloff: Falloff, m: &Metrics, p: Palette) {
    let h = m.row * 1.3;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(m.radius), p.bg);
    let pts: Vec<egui::Pos2> = (0..=48)
        .map(|i| {
            let t = i as f32 / 48.0;
            let v = falloff.eval(t);
            egui::pos2(
                rect.left() + 8.0 + (rect.width() - 16.0) * t,
                rect.bottom() - 6.0 - (rect.height() - 12.0) * v,
            )
        })
        .collect();
    painter.add(egui::Shape::line(pts, Stroke::new(1.8, p.accent)));
}

fn mask_controls(ui: &mut egui::Ui, st: &mut UiState, cx: &mut Ctx) {
    let m = cx.m;
    let (clear, invert) = two_up(
        ui,
        &m,
        |ui| button(ui, Icon::Close, "Clear", &m),
        |ui| button(ui, Icon::Mirror, "Invert", &m),
    );
    if clear {
        cx.actions.push(Action::ClearMask);
    }
    if invert {
        cx.actions.push(Action::InvertMask);
    }
    let (blur, sharpen) = two_up(
        ui,
        &m,
        |ui| button(ui, Icon::Smooth, "Blur", &m),
        |ui| button(ui, Icon::Crease, "Sharpen", &m),
    );
    if blur {
        cx.actions.push(Action::FilterMask(0.5));
    }
    if sharpen {
        cx.actions.push(Action::FilterMask(-0.5));
    }
    widgets::toggle(ui, &mut st.settings.show_mask, "Show the mask", m.row);
    BigSlider::new(&mut st.extract_thickness, 0.005..=0.4, "Extract thickness")
        .logarithmic(true)
        .decimals(3)
        .height(m.row)
        .show(ui);
    if widgets::wide_button(ui, Icon::Duplicate, "Extract to a new object", m.row, false).clicked() {
        cx.actions.push(Action::ExtractMask);
    }
}

/// Two equally wide controls side by side, handing back whatever each returned.
fn two_up<A, B>(
    ui: &mut egui::Ui,
    m: &Metrics,
    left: impl FnOnce(&mut egui::Ui) -> A,
    right: impl FnOnce(&mut egui::Ui) -> B,
) -> (A, B) {
    ui.horizontal(|ui| {
        let w = ((ui.available_width() - m.gap) * 0.5).max(36.0);
        let a = ui.scope(|ui| {
            ui.set_width(w);
            left(ui)
        });
        let b = ui.scope(|ui| {
            ui.set_width(w);
            right(ui)
        });
        (a.inner, b.inner)
    })
    .inner
}

/// A full-width button that reports whether it was pressed.
fn button(ui: &mut egui::Ui, icon: Icon, text: &str, m: &Metrics) -> bool {
    widgets::wide_button(ui, icon, text, m.row, false).clicked()
}

fn model_tab(ui: &mut egui::Ui, s: &mut Sculptor, st: &mut UiState, cx: &mut Ctx) {
    let (p, m) = (cx.p, cx.m);
    widgets::section_title(ui, "DYNAMIC TOPOLOGY");
    widgets::toggle(ui, &mut s.dyntopo_enabled, "Dynamic topology", m.row);
    let enabled = s.dyntopo_enabled;
    ui.add_enabled_ui(enabled, |ui| {
        // What the detail number is measured in. Changing it changes the unit,
        // so the value moves to that unit's own default rather than being read
        // as a length in one and a count of pixels in the next.
        let labels: Vec<&str> =
            sculpt_core::DetailMode::ALL.iter().map(|k| k.label()).collect();
        let current = sculpt_core::DetailMode::ALL
            .iter()
            .position(|k| *k == s.dyntopo.mode)
            .unwrap_or(0);
        if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
            let mode = sculpt_core::DetailMode::ALL[i];
            s.dyntopo.mode = mode;
            s.dyntopo.detail = mode.default_detail();
        }
        let (lo, hi) = s.dyntopo.mode.range();
        BigSlider::new(&mut s.dyntopo.detail, lo..=hi, "Detail size")
            .logarithmic(true)
            .decimals(if s.dyntopo.mode == sculpt_core::DetailMode::Screen { 1 } else { 4 })
            .suffix(s.dyntopo.mode.unit())
            .height(m.row)
            .show(ui);
        widgets::toggle(ui, &mut s.dyntopo.subdivide, "Subdivide under the brush", m.row);
        widgets::toggle(ui, &mut s.dyntopo.decimate, "Decimate under the brush", m.row);
        widgets::toggle(ui, &mut s.dyntopo.responsive, "Progressive refinement", m.row);
        ui.label(egui::RichText::new(
            "Progressive adds detail over more dabs to keep the brush responsive."
        ).small().color(p.dim));
        let mut millions = s.dyntopo.max_verts as f32 / 1.0e6;
        if BigSlider::new(&mut millions, 0.05..=8.0, "Vertex ceiling")
            .decimals(2)
            .suffix(" M")
            .height(m.row)
            .show(ui)
            .changed()
        {
            s.dyntopo.max_verts = (millions * 1.0e6) as usize;
        }
    });

    widgets::section_title(ui, "RESOLUTION");
    widgets::toggle(ui, &mut st.subdivide_smooth, "Smooth subdivision (Loop)", m.row);
    if widgets::wide_button(ui, Icon::Subdivide, "Subdivide the whole mesh", m.row, false).clicked()
    {
        cx.actions.push(Action::Subdivide(st.subdivide_smooth));
    }
    BigSlider::new(&mut st.decimate_ratio, 0.05..=0.95, "Keep")
        .decimals(2)
        .height(m.row)
        .show(ui);
    if widgets::wide_button(ui, Icon::Decimate, "Decimate", m.row, false).clicked() {
        cx.actions.push(Action::Decimate);
    }

    widgets::section_title(ui, "VOXEL REMESH");
    widgets::int_slider(ui, "Resolution", &mut st.remesh.resolution, 24..=350, &m);
    widgets::int_slider(ui, "Smoothing passes", &mut st.remesh.smoothing, 0..=6, &m);
    widgets::toggle(ui, &mut st.remesh.transfer_colors, "Keep colours", m.row);
    if widgets::wide_button(ui, Icon::Remesh, "Remesh", m.row, false).clicked() {
        cx.actions.push(Action::Remesh);
    }

    widgets::section_title(ui, "REPAIR");
    if widgets::wide_button(ui, Icon::CloseHoles, "Close holes", m.row, false).clicked() {
        cx.actions.push(Action::CloseHoles);
    }
    if widgets::wide_button(ui, Icon::Smooth, "Relax the whole mesh", m.row, false).clicked() {
        cx.actions.push(Action::SmoothAll);
    }

    widgets::section_title(ui, "SYMMETRY TOOLS");
    ui.horizontal(|ui| {
        let w = ((ui.available_width() - m.gap * 2.0) / 3.0).max(36.0);
        for axis in Axis::ALL {
            let hit = ui
                .scope(|ui| {
                    ui.set_width(w);
                    widgets::wide_button(ui, Icon::Mirror, axis.label(), m.row, false).clicked()
                })
                .inner;
            if hit {
                cx.actions.push(Action::Mirror(axis));
            }
        }
    });
    ui.label(
        egui::RichText::new("Symmetrize replaces one half with the mirror of the other.")
            .small()
            .color(p.dim),
    );
    let axis = s.symmetry_axis;
    let (keep_plus, keep_minus) = two_up(
        ui,
        &m,
        |ui| button(ui, Icon::Symmetry, "Keep +", &m),
        |ui| button(ui, Icon::Symmetry, "Keep -", &m),
    );
    if keep_plus {
        cx.actions.push(Action::Symmetrize(axis, true));
    }
    if keep_minus {
        cx.actions.push(Action::Symmetrize(axis, false));
    }

    widgets::section_title(ui, "MASK");
    mask_controls(ui, st, cx);
}

fn scene_tab(
    ui: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    giz: &mut Gizmo,
    cx: &mut Ctx,
) {
    let (p, m) = (cx.p, cx.m);
    widgets::section_title(ui, "OBJECTS");
    let active = s.scene.active;
    let count = s.scene.objects.len();
    for i in 0..count {
        let (name, visible) = {
            let o = &s.scene.objects[i];
            (o.name.clone(), o.visible)
        };
        let selected = i == active;
        ui.horizontal(|ui| {
            let icon = if visible { Icon::Eye } else { Icon::EyeOff };
            if widgets::icon_button(ui, icon, m.row, false, "Visibility").clicked() {
                cx.actions.push(Action::ToggleVisible(i));
            }
            let remaining = ui.available_width() - (m.row + m.gap) * 2.0;
            let hit = ui
                .scope(|ui| {
                    ui.set_width(remaining.max(50.0));
                    widgets::wide_button(ui, Icon::Remesh, &name, m.row, selected).clicked()
                })
                .inner;
            if hit {
                cx.actions.push(Action::SelectObject(i));
            }
            if widgets::icon_button(ui, Icon::Duplicate, m.row, false, "Duplicate").clicked() {
                cx.actions.push(Action::DuplicateObject(i));
            }
            ui.add_enabled_ui(count > 1, |ui| {
                if widgets::icon_button(ui, Icon::Trash, m.row, false, "Delete").clicked() {
                    cx.actions.push(Action::DeleteObject(i));
                }
            });
        });
    }

    ui.add_space(4.0);
    let merge = ui
        .add_enabled_ui(count > 1, |ui| {
            widgets::wide_button(ui, Icon::Remesh, "Merge everything visible", m.row, false)
                .clicked()
        })
        .inner;
    if merge {
        cx.actions.push(Action::MergeVisible);
    }

    widgets::section_title(ui, "ADD");
    let current = Primitive::ALL
        .iter()
        .position(|x| *x == st.add_primitive)
        .unwrap_or(0);
    egui::ComboBox::from_id_salt("prim")
        .selected_text(Primitive::ALL[current].label())
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for x in Primitive::ALL {
                ui.selectable_value(&mut st.add_primitive, x, x.label());
            }
        });
    let prim = st.add_primitive;
    let (add, replace) = two_up(
        ui,
        &m,
        |ui| button(ui, Icon::Plus, "Add", &m),
        |ui| button(ui, Icon::New, "Replace", &m),
    );
    if add {
        cx.actions.push(Action::AddObject(prim));
    }
    if replace {
        cx.actions.push(Action::New(prim));
    }

    widgets::section_title(ui, "GIZMO");
    let labels: Vec<&str> = GizmoMode::ALL.iter().map(|g| g.label()).collect();
    let current = GizmoMode::ALL
        .iter()
        .position(|g| *g == giz.mode)
        .unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
        giz.mode = GizmoMode::ALL[i];
    }
    ui.add_enabled_ui(giz.mode != GizmoMode::Off, |ui| {
        widgets::toggle(ui, &mut giz.local_space, "Follow the object's axes", m.row);
        BigSlider::new(&mut giz.size, 50.0..=180.0, "Handle size")
            .decimals(0)
            .height(m.row)
            .show(ui);
    });
    if giz.mode != GizmoMode::Off {
        ui.label(
            egui::RichText::new("Drag a handle to transform. The brush stays out of the way while the pointer is over one.")
                .small()
                .color(p.dim),
        );
    }

    widgets::section_title(ui, "PLACEMENT");
    if let Some(o) = s.scene.active_mut() {
        let mut pos = o.transform.position;
        let mut moved = false;
        for (label, value) in [("X", &mut pos.x), ("Y", &mut pos.y), ("Z", &mut pos.z)] {
            moved |= BigSlider::new(value, -3.0..=3.0, label)
                .decimals(3)
                .height(m.row)
                .show(ui)
                .changed();
        }
        let mut scale = o.transform.scale.x;
        let scaled = BigSlider::new(&mut scale, 0.05..=6.0, "Scale")
            .logarithmic(true)
            .decimals(3)
            .height(m.row)
            .show(ui)
            .changed();
        if moved {
            o.transform.position = pos;
        }
        if scaled {
            o.transform.scale = glam::Vec3::splat(scale);
        }
    }
    if widgets::wide_button(ui, Icon::Pin, "Bake the placement in", m.row, false).clicked() {
        cx.actions.push(Action::ApplyTransform);
    }

    widgets::section_title(ui, "FILES");
    if widgets::wide_button(ui, Icon::Open, "Import a mesh", m.row, false).clicked() {
        cx.actions.push(Action::Import);
    }
    if widgets::wide_button(ui, Icon::Save, "Export the active mesh", m.row, false).clicked() {
        cx.actions.push(Action::Export);
    }
    if widgets::wide_button(ui, Icon::Open, "Open a scene", m.row, false).clicked() {
        cx.actions.push(Action::OpenScene);
    }
    if widgets::wide_button(ui, Icon::Save, "Save the scene", m.row, false).clicked() {
        cx.actions.push(Action::SaveScene);
    }
    ui.label(
        egui::RichText::new("OBJ, PLY and STL for meshes; .sculpt keeps the whole scene.")
            .small()
            .color(p.dim),
    );
}

/// Where the matcap comes from, and how to take it apart.
fn matcap_controls(ui: &mut egui::Ui, st: &mut UiState, cx: &mut Ctx) {
    let (p, m) = (cx.p, cx.m);
    let labels: Vec<&str> = MatcapSource::ALL.iter().map(|s| s.label()).collect();
    let current = MatcapSource::ALL
        .iter()
        .position(|s| *s == st.matcap_source)
        .unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
        let chosen = MatcapSource::ALL[i];
        // Nothing to show yet is worse than the wrong thing: ask for the file
        // the moment the image source is picked with none loaded.
        if chosen == MatcapSource::Image && st.matcap_image.is_none() {
            cx.actions.push(Action::LoadMatcap);
        } else {
            st.matcap_source = chosen;
            cx.actions.push(Action::MatcapChanged);
        }
    }

    match st.matcap_source {
        MatcapSource::Preset => {
            egui::ComboBox::from_id_salt("matcap")
                .selected_text(st.matcap.label())
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for preset in matcap::Preset::ALL {
                        if ui.selectable_value(&mut st.matcap, preset, preset.label()).clicked() {
                            cx.actions.push(Action::MatcapChanged);
                        }
                    }
                });
            if widgets::wide_button(ui, Icon::Settings, "Take this one apart", m.row, false)
                .clicked()
            {
                st.lightcap = matcap::Lightcap::from_preset(st.matcap);
                st.matcap_source = MatcapSource::Lightcap;
                cx.actions.push(Action::MatcapChanged);
            }
        }
        MatcapSource::Lightcap => lightcap_editor(ui, st, cx),
        MatcapSource::Image => {
            ui.label(
                egui::RichText::new(match &st.matcap_image {
                    Some((size, _)) => format!("Loaded, {size} by {size}."),
                    None => "Nothing loaded yet.".to_string(),
                })
                .small()
                .color(p.dim),
            );
            if widgets::wide_button(ui, Icon::Open, "Load a matcap image", m.row, false).clicked() {
                cx.actions.push(Action::LoadMatcap);
            }
        }
    }
}

/// The material and the three lights behind a generated matcap.
fn lightcap_editor(ui: &mut egui::Ui, st: &mut UiState, cx: &mut Ctx) {
    let (p, m) = (cx.p, cx.m);
    let mut changed = false;
    let lc = &mut st.lightcap;

    changed |= vec_color_row(ui, "Surface", &mut lc.look.base, m);
    changed |= vec_color_row(ui, "Highlight", &mut lc.look.spec, m);
    changed |= vec_color_row(ui, "Rim", &mut lc.look.rim, m);
    changed |= BigSlider::new(&mut lc.look.shininess, 2.0..=180.0, "Tightness")
        .decimals(0)
        .height(m.row)
        .show(ui)
        .changed();
    changed |= BigSlider::new(&mut lc.look.spec_amt, 0.0..=1.0, "Gloss")
        .height(m.row)
        .show(ui)
        .changed();
    changed |= BigSlider::new(&mut lc.look.ambient, 0.0..=1.0, "Ambient")
        .height(m.row)
        .show(ui)
        .changed();

    for (i, light) in lc.lights.iter_mut().enumerate() {
        widgets::section_title(ui, ["KEY", "FILL", "BOUNCE"][i]);
        changed |= widgets::toggle(ui, &mut light.on, "On", m.row).clicked();
        ui.add_enabled_ui(light.on, |ui| {
            ui.horizontal(|ui| {
                changed |= light_dial(ui, &mut light.dir, m.row * 2.6, p);
                ui.vertical(|ui| {
                    changed |= vec_color_row(ui, "Colour", &mut light.color, m);
                    changed |= BigSlider::new(&mut light.power, 0.0..=2.0, "Power")
                        .height(m.row)
                        .show(ui)
                        .changed();
                });
            });
        });
    }

    ui.label(
        egui::RichText::new(
            "Drag inside a dial to aim its light. The dial is the model as you see it: the middle points straight at you, the rim is edge on.",
        )
        .small()
        .color(p.dim),
    );
    if widgets::wide_button(ui, Icon::Reset, "Back to the preset", m.row, false).clicked() {
        st.lightcap = matcap::Lightcap::from_preset(st.matcap);
        changed = true;
    }
    if changed {
        cx.actions.push(Action::MatcapChanged);
    }
}

/// A colour row over a `Vec3`, which is how the lighting stores its colours.
fn vec_color_row(ui: &mut egui::Ui, label: &str, c: &mut glam::Vec3, m: Metrics) -> bool {
    let mut rgb = [c.x, c.y, c.z];
    let changed = widgets::color_row(ui, label, &mut rgb, m.row).changed();
    if changed {
        *c = glam::Vec3::from_array(rgb);
    }
    changed
}

/// A disc you drag to aim a light, drawn as the lit sphere it acts on.
///
/// The obvious alternative is two angle sliders, which nobody can read: this
/// shows the model from where the artist is sitting, so the light goes where it
/// is pointed rather than where the numbers said.
fn light_dial(ui: &mut egui::Ui, dir: &mut glam::Vec3, size: f32, p: Palette) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::drag());
    let center = rect.center();
    let radius = size * 0.5 - 2.0;
    let mut changed = false;

    if response.dragged() || response.drag_started() {
        if let Some(at) = ui.ctx().pointer_interact_pos() {
            let mut d = (at - center) / radius;
            // Past the rim the light is behind the model, so the direction is
            // clamped to the silhouette rather than wrapping round.
            let len = d.length();
            if len > 1.0 {
                d /= len;
            }
            let z = (1.0f32 - d.length() * d.length()).max(0.0).sqrt();
            *dir = glam::Vec3::new(d.x, -d.y, z).normalize_or(glam::Vec3::Z);
            changed = true;
        }
    }

    let painter = ui.painter();
    painter.circle(center, radius, p.bg, Stroke::new(1.0, p.line));
    // A few rings to read the tilt against.
    for t in [0.33, 0.66] {
        painter.circle_stroke(center, radius * t, Stroke::new(1.0, p.line.gamma_multiply(0.5)));
    }
    let n = dir.normalize_or(glam::Vec3::Z);
    let at = egui::Pos2::new(center.x + n.x * radius, center.y - n.y * radius);
    painter.line_segment([center, at], Stroke::new(1.0, p.accent.gamma_multiply(0.5)));
    painter.circle(at, 5.0, p.accent, Stroke::new(1.0, p.bg));
    changed
}

fn view_tab(ui: &mut egui::Ui, st: &mut UiState, cam: &mut Camera, cx: &mut Ctx) {
    let (p, m) = (cx.p, cx.m);
    widgets::section_title(ui, "SHADING");
    let labels: Vec<&str> = Shading::ALL.iter().map(|x| x.label()).collect();
    let current = Shading::ALL
        .iter()
        .position(|x| *x == st.settings.shading)
        .unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
        st.settings.shading = Shading::ALL[i];
    }

    if matches!(st.settings.shading, Shading::Matcap | Shading::Cavity) {
        matcap_controls(ui, st, cx);
    }
    if st.settings.shading == Shading::Cavity {
        BigSlider::new(&mut st.settings.cavity_strength, 0.5..=40.0, "Cavity")
            .decimals(1)
            .height(m.row)
            .show(ui);
    }

    widgets::section_title(ui, "OCCLUSION");
    widgets::toggle(ui, &mut st.settings.occlusion, "Shade the creases", m.row);
    ui.add_enabled_ui(st.settings.occlusion, |ui| {
        BigSlider::new(&mut st.settings.occlusion_radius, 0.01..=0.6, "Reach")
            .logarithmic(true)
            .decimals(3)
            .height(m.row)
            .show(ui);
        BigSlider::new(&mut st.settings.occlusion_strength, 0.0..=2.0, "Depth")
            .decimals(2)
            .height(m.row)
            .show(ui);
    });

    // The tone curve, where there is light to map. The other views are colours
    // somebody already chose, so there is nothing to grade.
    if matches!(st.settings.shading, Shading::Pbr | Shading::Clay) {
        widgets::section_title(ui, "TONE");
        widgets::toggle(ui, &mut st.settings.tone, "Roll off the highlights", m.row);
        ui.add_enabled_ui(st.settings.tone, |ui| {
            BigSlider::new(&mut st.settings.exposure, -3.0..=3.0, "Exposure")
                .decimals(2)
                .suffix(" stops")
                .height(m.row)
                .show(ui);
            BigSlider::new(&mut st.settings.contrast, 0.5..=2.0, "Contrast")
                .decimals(2)
                .height(m.row)
                .show(ui);
            BigSlider::new(&mut st.settings.saturation, 0.0..=2.0, "Saturation")
                .decimals(2)
                .height(m.row)
                .show(ui);
        });
        widgets::section_title(ui, "SURFACE");
    }

    widgets::toggle(
        ui,
        &mut st.settings.backface_cull,
        "Hide faces turned away",
        m.row,
    );
    widgets::toggle(ui, &mut st.settings.flat, "Flat shading", m.row);
    widgets::toggle(ui, &mut st.settings.vertex_color, "Vertex colours", m.row);
    let wire_ok = st.wireframe_available;
    ui.add_enabled_ui(wire_ok, |ui| {
        widgets::toggle(ui, &mut st.settings.wireframe, "Wireframe   W", m.row);
    });
    if !wire_ok {
        ui.label(
            egui::RichText::new("This GPU has no line polygon mode.")
                .small()
                .color(p.dim),
        );
    }
    BigSlider::new(&mut st.settings.opacity, 0.15..=1.0, "Opacity")
        .decimals(2)
        .height(m.row)
        .show(ui);

    widgets::section_title(ui, "SCENE");
    widgets::toggle(ui, &mut st.settings.grid, "Ground grid   G", m.row);
    let grid = st.settings.grid;
    ui.add_enabled_ui(grid, |ui| {
        BigSlider::new(&mut st.settings.grid_spacing, 0.02..=2.0, "Grid spacing")
            .logarithmic(true)
            .decimals(3)
            .height(m.row)
            .show(ui);
    });
    widgets::color_row(ui, "Background top", &mut st.settings.background_top, m.row);
    widgets::color_row(
        ui,
        "Background bottom",
        &mut st.settings.background_bottom,
        m.row,
    );

    widgets::section_title(ui, "CAMERA");
    let proj_current = if cam.projection == Projection::Perspective { 0 } else { 1 };
    if let Some(i) = widgets::segmented(ui, &["Perspective", "Ortho"], proj_current, m.row) {
        cam.projection = if i == 0 {
            Projection::Perspective
        } else {
            Projection::Orthographic
        };
    }
    let mut fov_deg = cam.fov_y.to_degrees();
    if BigSlider::new(&mut fov_deg, 15.0..=100.0, "Field of view")
        .decimals(0)
        .height(m.row)
        .show(ui)
        .changed()
    {
        cam.fov_y = fov_deg.to_radians();
    }
    BigSlider::new(&mut cam.smoothing, 0.0..=0.95, "Camera smoothing")
        .decimals(2)
        .height(m.row)
        .show(ui);
    let mut speed = cam.orbit_speed * 1000.0;
    if BigSlider::new(&mut speed, 2.0..=20.0, "Orbit speed")
        .decimals(1)
        .height(m.row)
        .show(ui)
        .changed()
    {
        cam.orbit_speed = speed / 1000.0;
    }
    widgets::toggle(ui, &mut cam.invert_orbit_y, "Invert vertical orbit", m.row);

    ui.add_space(4.0);
    let preset_labels: Vec<&str> = ViewPreset::ALL.iter().map(|x| x.label()).collect();
    if let Some(i) = widgets::segmented(ui, &preset_labels, usize::MAX, m.row) {
        cx.actions.push(Action::SetView(ViewPreset::ALL[i]));
    }
    if widgets::wide_button(ui, Icon::FrameView, "Frame the model   F", m.row, false).clicked() {
        cx.actions.push(Action::FrameView);
    }

    widgets::section_title(ui, "PERFORMANCE");
    widgets::toggle(ui, &mut st.settings.gpu_scatter, "GPU vertex upload", m.row);
    ui.label(
        egui::RichText::new(
            "A stroke touches a scattered handful of vertices. With this on, only those are sent and a compute pass puts them in place; with it off, the whole mesh goes across every frame.",
        )
        .small()
        .color(p.dim),
    );
    ui.label(
        egui::RichText::new(format!("Last frame sent {}", format_bytes(st.upload_bytes)))
            .small()
            .monospace()
            .color(p.faint),
    );

    widgets::section_title(ui, "ANTI-ALIASING");
    // Only offer what this GPU actually accepts: asking for a sample count the
    // depth buffer cannot do is a validation error, not a soft failure.
    let msaa_ok = st.msaa_available;
    let counts: Vec<u32> = [1u32, 2, 4, 8]
        .into_iter()
        .filter(|c| *c <= st.max_samples)
        .collect();
    let labels: Vec<&str> = counts
        .iter()
        .map(|c| match c {
            1 => "Off",
            2 => "2x",
            4 => "4x",
            _ => "8x",
        })
        .collect();
    let picked = ui
        .add_enabled_ui(msaa_ok, |ui| {
            let current = counts.iter().position(|c| *c == st.sample_count).unwrap_or(0);
            widgets::segmented(ui, &labels, current, m.row).map(|i| counts[i])
        })
        .inner;
    if let Some(n) = picked {
        st.sample_count = n;
        cx.actions.push(Action::SampleCountChanged(n));
    }
    ui.add_enabled_ui(msaa_ok, |ui| {
        widgets::toggle(ui, &mut st.adaptive_msaa, "Drop it on dense models", m.row);
    });
    ui.label(
        egui::RichText::new(
            "Past a few million triangles the triangles are smaller than a pixel, so the picture is already smooth and multisampling only multiplies the work.",
        )
        .small()
        .color(p.dim),
    );
}

fn interface_tab(ui: &mut egui::Ui, st: &mut UiState, cx: &mut Ctx) {
    let (p, m) = (cx.p, cx.m);
    ui.label("Layout, brushes, imported alphas and lighting are restored on the next launch.");
    if widgets::wide_button(ui, Icon::Save, "Save workspace now", m.row, false).clicked() {
        cx.actions.push(Action::SaveWorkspace);
    }
    BigSlider::new(&mut st.preview_opacity, 0.0..=0.8, "Brush preview opacity")
        .decimals(2).height(m.row).show(ui);
    let t = &mut st.theme;

    widgets::section_title(ui, "COLOUR");
    let preset_labels: Vec<&str> = ColorPreset::ALL.iter().map(|x| x.label()).collect();
    let current = ColorPreset::ALL
        .iter()
        .position(|x| x.accent() == t.accent)
        .unwrap_or(usize::MAX);
    if let Some(i) = widgets::segmented(ui, &preset_labels, current, m.row) {
        t.accent = ColorPreset::ALL[i].accent();
    }
    widgets::color_row(ui, "Accent", &mut t.accent, m.row);
    widgets::color_row(ui, "Background", &mut t.base, m.row);
    widgets::color_row(ui, "Text", &mut t.text, m.row);
    BigSlider::new(&mut t.contrast, 0.0..=1.0, "Contrast")
        .decimals(2)
        .height(m.row)
        .show(ui);

    widgets::section_title(ui, "SIZE");
    widgets::toggle(ui, &mut t.touch, "Touch layout", m.row);

    // Zooming the interface moves this very slider, so the drag runs against a
    // draft and only lands when the pointer is released.
    let scale = BigSlider::new(&mut st.ui_scale_draft, 0.6..=2.2, "Interface scale")
        .decimals(2)
        .height(m.row)
        .show(ui);
    if scale.drag_stopped() || (scale.changed() && !scale.dragged()) {
        st.theme.ui_scale = st.ui_scale_draft;
    }
    if scale.dragged() && (st.ui_scale_draft - st.theme.ui_scale).abs() > 0.005 {
        ui.label(
            egui::RichText::new(format!("release to apply {:.2}", st.ui_scale_draft))
                .small()
                .color(p.accent),
        );
    }

    let t = &mut st.theme;
    BigSlider::new(&mut t.text_scale, 0.7..=1.8, "Text size")
        .decimals(2)
        .height(m.row)
        .show(ui);
    BigSlider::new(&mut t.icon_scale, 0.6..=1.6, "Icon size")
        .decimals(2)
        .height(m.row)
        .show(ui);
    BigSlider::new(&mut t.tool_size, 32.0..=96.0, "Tool tile")
        .decimals(0)
        .height(m.row)
        .show(ui);
    BigSlider::new(&mut t.row_height, 22.0..=64.0, "Row height")
        .decimals(0)
        .height(m.row)
        .show(ui);
    BigSlider::new(&mut t.panel_width, 220.0..=560.0, "Dock width")
        .decimals(0)
        .height(m.row)
        .show(ui);
    let mut radius = t.corner_radius as f32;
    if BigSlider::new(&mut radius, 0.0..=20.0, "Corner radius")
        .decimals(0)
        .height(m.row)
        .show(ui)
        .changed()
    {
        t.corner_radius = radius as u8;
    }
    BigSlider::new(&mut t.scrollbar, 6.0..=22.0, "Scroll bar")
        .decimals(0)
        .height(m.row)
        .show(ui);

    widgets::section_title(ui, "FLOATING CONTROLS");
    widgets::toggle(ui, &mut st.show_nav, "Orientation ball", m.row);
    widgets::toggle(ui, &mut st.hud.arrange, "Arrange: drag them anywhere", m.row);
    if st.hud.arrange {
        ui.label(
            egui::RichText::new("Drag any button to move it. Turn this off to use them again.")
                .small()
                .color(p.accent),
        );
    }
    // Each button gets its own line: shown or not, and how big.
    for pod in st.hud.pods.iter_mut() {
        ui.horizontal(|ui| {
            let icon = if pod.visible { Icon::Eye } else { Icon::EyeOff };
            if widgets::icon_button(ui, icon, m.row, false, "Show or hide").clicked() {
                pod.visible = !pod.visible;
            }
            ui.scope(|ui| {
                ui.set_width(ui.available_width().max(60.0));
                BigSlider::new(&mut pod.size, 30.0..=110.0, pod.kind.label())
                    .decimals(0)
                    .height(m.row)
                    .show(ui);
            });
        });
    }
    if widgets::wide_button(ui, Icon::Reset, "Put them back", m.row, false).clicked() {
        st.hud.reset();
    }

    widgets::section_title(ui, "INPUT");
    ui.label(
        egui::RichText::new("What each device does when you drag with it.")
            .small()
            .color(p.dim),
    );
    let labels: Vec<&str> = Role::ALL.iter().map(|r| r.label()).collect();
    for (name, role) in [
        ("Left button", &mut st.bindings.left),
        ("Middle button", &mut st.bindings.middle),
        ("Right button", &mut st.bindings.right),
        ("One finger", &mut st.bindings.touch),
        ("Pen", &mut st.bindings.pen),
    ] {
        ui.label(egui::RichText::new(name).small().color(p.dim));
        let current = Role::ALL.iter().position(|r| r == role).unwrap_or(0);
        if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
            *role = Role::ALL[i];
        }
        ui.label(egui::RichText::new(role.hint()).small().color(p.faint));
    }
    ui.label(
        egui::RichText::new(
            "Auto asks the model: a press that lands on it draws, a press that lands off it spins the view, and the answer holds for the whole drag. The brush reach counts as on the model, so the silhouette is still grabbable.",
        )
        .small()
        .color(p.dim),
    );
    ui.label(
        egui::RichText::new(
            "Alt with the left button always orbits, whatever the buttons are set to, and shift turns an orbit into a pan.",
        )
        .small()
        .color(p.dim),
    );

    widgets::section_title(ui, "RADIAL MENU");
    ui.label(
        egui::RichText::new(
            "Hold the key and the tools, the size and force pad, and the colour wheel come to the cursor. Let go and it is gone.",
        )
        .small()
        .color(p.dim),
    );
    let rebinding = st.rebinding_wheel;
    let label = if rebinding {
        "press any key...".to_string()
    } else {
        format!("Key: {}", key_name(st.wheel_key))
    };
    if widgets::wide_button(ui, Icon::Settings, &label, m.row, rebinding).clicked() {
        st.rebinding_wheel = !st.rebinding_wheel;
    }
    widgets::toggle(ui, &mut st.wheel.show_colors, "Include the colour wheel", m.row);
    BigSlider::new(&mut st.wheel.size, 110.0..=260.0, "Menu size")
        .decimals(0)
        .height(m.row)
        .show(ui);

    ui.label(
        egui::RichText::new(
            "Where the menu appears when the key summons it. From the floating button it always opens in the middle, and so does the brush preview.",
        )
        .small()
        .color(p.dim),
    );
    let labels: Vec<&str> = Anchor::ALL.iter().map(|a| a.label()).collect();
    let current = Anchor::ALL.iter().position(|a| *a == st.anchor).unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
        st.anchor = Anchor::ALL[i];
    }

    widgets::section_title(ui, "LAYOUT");
    ui.label(egui::RichText::new("Tool rail").small().color(p.dim));
    let side_labels: Vec<&str> = Side::ALL.iter().map(|x| x.label()).collect();
    let current = Side::ALL.iter().position(|x| *x == t.rail_side).unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, &side_labels, current, m.row) {
        t.rail_side = Side::ALL[i];
    }
    ui.label(egui::RichText::new("Settings dock").small().color(p.dim));
    let dock_labels: Vec<&str> = Side::SIDES.iter().map(|x| x.label()).collect();
    let current = Side::SIDES.iter().position(|x| *x == t.panel_side).unwrap_or(1);
    if let Some(i) = widgets::segmented(ui, &dock_labels, current, m.row) {
        t.panel_side = Side::SIDES[i];
    }
    widgets::toggle(ui, &mut st.show_rail, "Show the tool rail", m.row);
    widgets::toggle(ui, &mut st.show_stats, "Show statistics", m.row);

    ui.add_space(8.0);
    if widgets::wide_button(ui, Icon::Reset, "Reset the theme", m.row, false).clicked() {
        cx.actions.push(Action::ResetTheme);
    }
}

// ---------------------------------------------------------------------------
// viewport overlays
// ---------------------------------------------------------------------------

fn viewport_overlay(
    root: &mut egui::Ui,
    s: &Sculptor,
    st: &mut UiState,
    overlay: &Overlay,
    p: Palette,
) {
    let ctx = root.ctx().clone();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("viewport_overlay"),
    )).with_clip_rect(st.viewport);

    if let Some(corners) = overlay.footprint.filter(|_| st.preview_opacity > 0.0) {
        let key = (s.brush.falloff, s.brush.alpha, s.alphas.len());
        if st.preview_key != Some(key) {
            const N: usize = 96;
            let alpha = s.brush.alpha.and_then(|i| s.alphas.get(i as usize));
            let pixels = (0..N * N).map(|i| {
                let u = (i % N) as f32 / (N - 1) as f32;
                let v = (i / N) as f32 / (N - 1) as f32;
                let radius = egui::vec2(u * 2.0 - 1.0, v * 2.0 - 1.0).length();
                let weight = if radius >= 1.0 { 0.0 } else {
                    s.brush.falloff.eval(radius) * alpha.map_or(1.0, |a| a.sample(u, v))
                };
                Color32::from_white_alpha((weight * 255.0) as u8)
            }).collect();
            let image = egui::ColorImage { size: [N, N], pixels, source_size: egui::vec2(N as f32, N as f32) };
            if let Some(texture) = &mut st.preview_texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                st.preview_texture = Some(ctx.load_texture("brush footprint", image, egui::TextureOptions::LINEAR));
            }
            st.preview_key = Some(key);
        }
        if let Some(texture) = &st.preview_texture {
            let tint = if s.brush.negative { Color32::from_rgb(255, 140, 100) } else { p.accent };
            let color = tint.gamma_multiply(st.preview_opacity);
            let mut mesh = egui::Mesh::with_texture(texture.id());
            for (pos, uv) in corners.into_iter().zip([
                egui::pos2(0.0, 0.0), egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0), egui::pos2(0.0, 1.0),
            ]) {
                mesh.vertices.push(egui::epaint::Vertex { pos, uv, color });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            painter.add(egui::Shape::mesh(mesh));
        }
    }

    // Brush cursor: outer ring is the radius, inner ring the strength.
    if let Some((pos, radius)) = overlay.cursor {
        let accent = if overlay.stroking {
            p.accent
        } else {
            Color32::from_white_alpha(200)
        };
        painter.circle_stroke(pos, radius, Stroke::new(1.5, accent));
        let inner = radius * s.brush.strength.clamp(0.05, 1.0);
        painter.circle_stroke(pos, inner, Stroke::new(1.0, Color32::from_white_alpha(70)));
        if let Some((nx, ny)) = overlay.cursor_tilt {
            painter.line_segment(
                [pos, egui::pos2(pos.x + nx * radius, pos.y + ny * radius)],
                Stroke::new(1.0, Color32::from_white_alpha(110)),
            );
        }
        // Off the silhouette the brush still bites, just not under the pointer.
        // Marking the spot is the difference between a tool that feels broken
        // and one that feels like it is reaching.
        if let Some(anchor) = overlay.anchor {
            let mark = Color32::from_rgb(240, 90, 90);
            painter.line_segment([pos, anchor], Stroke::new(1.0, mark.gamma_multiply(0.45)));
            painter.circle_stroke(anchor, 7.0, Stroke::new(1.2, mark.gamma_multiply(0.8)));
            painter.circle_filled(anchor, 2.0, mark);
        }
    }

    // The statistics belong to the viewport, not the window: measured against
    // the window they slide under the tool rail.
    let rect = st.viewport;

    if st.show_stats {
        let text = format!(
            "{} verts   {} tris\nundo {:.0} MB   {} objects\nupload {}",
            s.mesh().vert_count(),
            s.mesh().face_count(),
            s.history.used_bytes() as f64 / 1.0e6,
            s.scene.objects.len(),
            format_bytes(st.upload_bytes),
        );
        painter.text(
            egui::pos2(rect.left() + 12.0, rect.bottom() - 12.0),
            Align2::LEFT_BOTTOM,
            text,
            egui::FontId::monospace(11.0),
            p.faint,
        );
    }

    // Transient status message, bottom centre.
    if !st.status.is_empty() && st.status_age < 4.0 {
        let alpha = (4.0 - st.status_age).clamp(0.0, 1.0);
        let color = p.text.gamma_multiply(alpha);
        let galley =
            painter.layout_no_wrap(st.status.clone(), egui::FontId::proportional(13.0), color);
        let bg = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.bottom() - 26.0),
            galley.size() + Vec2::new(22.0, 12.0),
        );
        painter.rect_filled(bg, CornerRadius::same(10), p.raised.gamma_multiply(alpha * 0.95));
        painter.galley(
            egui::pos2(
                bg.center().x - galley.size().x * 0.5,
                bg.center().y - galley.size().y * 0.5,
            ),
            galley,
            color,
        );
    }

    if st.show_help {
        help_window(&ctx, st, p);
    }
}

fn help_window(ctx: &egui::Context, st: &mut UiState, p: Palette) {
    let mut open = st.show_help;
    egui::Window::new("Shortcuts")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .frame(
            Frame::window(&ctx.style_of(egui::Theme::Dark))
                .fill(p.panel)
                .stroke(Stroke::new(1.0, p.line)),
        )
        .show(ctx, |ui| {
            let rows: [(&str, &str); 15] = [
                ("Left mouse, one finger", "Sculpt"),
                ("Middle or right mouse", "Orbit"),
                ("Two fingers", "Orbit, pinch to zoom, drag together to pan"),
                ("Three fingers", "Pan"),
                ("Shift + middle mouse", "Pan"),
                ("Wheel", "Zoom"),
                ("Alt + left mouse", "Orbit without sculpting"),
                ("Ctrl (hold)", "Invert the brush"),
                ("Shift (hold)", "Smooth instead of sculpting"),
                ("[ and ]", "Brush radius"),
                ("- and =", "Brush strength"),
                ("1 to 0, Shift+1..3", "Pick a tool"),
                ("X / W / G / F", "Symmetry, wireframe, grid, frame"),
                ("Tab / H", "Dock, shortcuts"),
                ("Numpad 1 / 3 / 7 / 5", "Front, right, top, orthographic"),
            ];
            for (key, what) in rows {
                ui.horizontal(|ui| {
                    ui.scope(|ui| {
                        // A share of what there is rather than a fixed column:
                        // a hard number here is a floor the whole panel cannot
                        // go below once it is floating.
                        ui.set_width((ui.available_width() * 0.55).clamp(80.0, 210.0));
                        ui.label(egui::RichText::new(key).monospace().small().color(p.accent));
                    });
                    ui.label(egui::RichText::new(what).small().color(p.text));
                });
            }
        });
    st.show_help = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every scheme has to come back with colours that are actually different
    /// from the one in hand, or the row of chips is decoration.
    #[test]
    fn a_harmony_offers_something_new() {
        let (h, s, v) = (0.1f32, 0.8f32, 0.7f32);
        let (r, g, b) = wheel::hsv_to_rgb(h, s, v);
        let base = glam::Vec3::new(r, g, b);
        for harmony in Harmony::ALL {
            let mates = harmony.mates(h, s, v);
            if harmony == Harmony::None {
                assert!(mates.is_empty());
                continue;
            }
            assert!(!mates.is_empty(), "{harmony:?} offered nothing");
            for c in mates {
                assert!((c - base).length() > 0.05, "{harmony:?} repeated the colour");
                assert!(
                    c.cmpge(glam::Vec3::ZERO).all() && c.cmple(glam::Vec3::ONE).all(),
                    "{harmony:?} left the range"
                );
            }
        }
    }

    /// The opposite of the opposite is where you started.
    #[test]
    fn the_complement_is_its_own_inverse() {
        let (h, s, v) = (0.2f32, 0.9f32, 0.8f32);
        let there = Harmony::Complement.mates(h, s, v)[0];
        let (h2, s2, v2) = wheel::rgb_to_hsv(there.x, there.y, there.z);
        let back = Harmony::Complement.mates(h2, s2, v2)[0];
        let (r, g, b) = wheel::hsv_to_rgb(h, s, v);
        assert!((back - glam::Vec3::new(r, g, b)).length() < 1e-3);
    }

    /// A press on a floating control belongs to the interface even though it is
    /// inside the viewport, and a press on bare viewport does not.
    ///
    /// This is what a pen gets wrong when the question is put to egui instead:
    /// it has no hover, so at the moment of contact egui has never seen the
    /// pointer there and says the interface does not want it.
    #[test]
    fn a_floating_control_owns_the_press_under_it() {
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let button = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(60.0, 60.0));

        let mut input = egui::RawInput::default();
        input.screen_rect = Some(viewport);
        // Two frames: the first registers the area, the second can be asked
        // about it, which is exactly how it goes in the application.
        for _ in 0..2 {
            ctx.begin_pass(input.clone());
            egui::Area::new(egui::Id::new("pod"))
                .fixed_pos(button.min)
                .order(egui::Order::Middle)
                .show(&ctx, |ui| {
                    ui.allocate_exact_size(button.size(), egui::Sense::click_and_drag());
                });
            // The font atlas comes out as a texture delta, and epaint refuses
            // to be dropped with one unhandled. There is no renderer here to
            // hand it to.
            let mut out = ctx.end_pass();
            out.textures_delta.clear();
        }

        assert!(interface_owns(&ctx, viewport, button.center()));
        assert!(!interface_owns(&ctx, viewport, egui::pos2(600.0, 400.0)));
        // Outside the viewport is a dock, whatever egui thinks.
        assert!(interface_owns(&ctx, viewport, egui::pos2(900.0, 300.0)));
    }

    /// A tab that is floating must not also be listed in the dock, and closing
    /// its window has to put it back rather than lose it.
    #[test]
    fn a_floating_tab_leaves_the_dock() {
        let mut st = UiState::default();
        st.floating.push(Tab::Colour);
        let docked: Vec<Tab> = Tab::ALL
            .iter()
            .copied()
            .filter(|t| !st.floating.contains(t))
            .collect();
        assert!(!docked.contains(&Tab::Colour));
        assert_eq!(docked.len(), Tab::ALL.len() - 1);
    }
}
