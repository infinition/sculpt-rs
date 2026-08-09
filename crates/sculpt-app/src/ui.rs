//! The interface: top bar, brush rail, settings dock, viewport overlays.
//!
//! Layout is built around one idea: the tool you are holding is always one
//! glance away, its settings are always in the same dock, and everything you
//! can touch is at least a fingertip wide. Where those docks sit, how big they
//! are and what colour everything is are all yours to change from the UI tab.

use crate::camera::{Camera, Projection, ViewPreset};
use crate::gizmo::{Gizmo, GizmoMode};
use crate::icons::Icon;
use crate::matcap;
use crate::renderer::{FrameSettings, Shading};
use crate::theme::{ColorPreset, Metrics, Palette, Side, UiTheme};
use crate::widgets::{self, BigSlider};
use egui::{Align2, Color32, CornerRadius, Frame, Margin, Sense, Stroke, Vec2};
use sculpt_core::{Axis, BlendMode, BrushKind, Falloff, FillScope, RemeshOptions, Sculptor};

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
    ResetBrushes,
    ResetTheme,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Brush,
    Model,
    Scene,
    View,
    Interface,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Brush, Tab::Model, Tab::Scene, Tab::View, Tab::Interface];

    fn label(self) -> &'static str {
        match self {
            Tab::Brush => "Brush",
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
    pub fps: f32,
    pub status: String,
    pub status_age: f32,
    pub remesh: RemeshOptions,
    pub decimate_ratio: f32,
    pub extract_thickness: f32,
    pub subdivide_smooth: bool,
    pub sample_count: u32,
    pub msaa_available: bool,
    pub wireframe_available: bool,
    pub picking_color: bool,
    pub add_primitive: Primitive,
    /// Bytes the last frame sent to the GPU, shown in the statistics.
    pub upload_bytes: u64,
    /// Interface scale being dragged, applied when the drag ends. Zooming
    /// rescales the coordinate space the slider itself lives in, so committing
    /// mid-gesture would move the rail out from under the finger.
    pub ui_scale_draft: f32,
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
            fps: 0.0,
            status: String::new(),
            status_age: 0.0,
            remesh: RemeshOptions::default(),
            decimate_ratio: 0.5,
            extract_thickness: 0.05,
            subdivide_smooth: true,
            sample_count: 4,
            msaa_available: true,
            wireframe_available: true,
            picking_color: false,
            add_primitive: Primitive::Sphere,
            upload_bytes: 0,
            ui_scale_draft: UiTheme::default().ui_scale,
        }
    }
}

impl UiState {
    pub fn say(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_age = 0.0;
    }
}

/// What the viewport should paint on top of the 3D image.
pub struct Overlay {
    /// Cursor centre and radius, in points.
    pub cursor: Option<(egui::Pos2, f32)>,
    /// Surface normal under the cursor, projected to screen, for the tilt spoke.
    pub cursor_tilt: Option<(f32, f32)>,
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
    viewport_overlay(root, s, st, overlay, p);

    actions
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
                        egui::RichText::new(format!("{:>3.0} fps", st.fps))
                            .small()
                            .monospace()
                            .color(if st.fps < 25.0 { p.warn } else { p.faint }),
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
            scroll.auto_shrink([false, false]).show(ui, |ui| {
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
    dock("panel", st.theme.panel_side)
        .default_size(m.panel_width)
        .size_range(200.0..=680.0)
        .resizable(true)
        .frame(
            Frame::new()
                .fill(p.panel)
                .inner_margin(Margin::same(m.pad as i8)),
        )
        .show(root, |ui| {
            let labels: Vec<&str> = Tab::ALL.iter().map(|t| t.label()).collect();
            let current = Tab::ALL.iter().position(|t| *t == st.tab).unwrap_or(0);
            if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
                st.tab = Tab::ALL[i];
            }
            ui.add_space(6.0);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match st.tab {
                    Tab::Brush => brush_tab(ui, s, st, cx),
                    Tab::Model => model_tab(ui, s, st, cx),
                    Tab::Scene => scene_tab(ui, s, st, giz, cx),
                    Tab::View => view_tab(ui, st, cam, cx),
                    Tab::Interface => interface_tab(ui, st, cx),
                });
        });
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

    widgets::section_title(ui, "BEHAVIOUR");
    widgets::toggle(ui, &mut s.brush.culling, "Front faces only", m.row);
    widgets::toggle(ui, &mut s.brush.lock_plane, "Lock the stroke plane", m.row);
    BigSlider::new(&mut s.brush.auto_smooth, 0.0..=1.0, "Auto smooth")
        .height(m.row)
        .show(ui);

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

    ui.add_space(8.0);
    if widgets::wide_button(ui, Icon::Reset, "Reset every tool", m.row, false).clicked() {
        cx.actions.push(Action::ResetBrushes);
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
        BigSlider::new(&mut s.dyntopo.detail, 0.002..=0.3, "Detail size")
            .logarithmic(true)
            .decimals(4)
            .height(m.row)
            .show(ui);
        widgets::toggle(ui, &mut s.dyntopo.subdivide, "Subdivide under the brush", m.row);
        widgets::toggle(ui, &mut s.dyntopo.decimate, "Decimate under the brush", m.row);
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
    }
    if st.settings.shading == Shading::Cavity {
        BigSlider::new(&mut st.settings.cavity_strength, 0.5..=40.0, "Cavity")
            .decimals(1)
            .height(m.row)
            .show(ui);
    }

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
    let msaa_ok = st.msaa_available;
    let picked = ui
        .add_enabled_ui(msaa_ok, |ui| {
            let counts = [1u32, 2, 4, 8];
            let current = counts.iter().position(|c| *c == st.sample_count).unwrap_or(2);
            widgets::segmented(ui, &["Off", "2x", "4x", "8x"], current, m.row)
                .map(|i| counts[i])
        })
        .inner;
    if let Some(n) = picked {
        st.sample_count = n;
        cx.actions.push(Action::SampleCountChanged(n));
    }
}

fn interface_tab(ui: &mut egui::Ui, st: &mut UiState, cx: &mut Ctx) {
    let (p, m) = (cx.p, cx.m);
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
    ));

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
    }

    let rect = ctx.content_rect();

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
                        ui.set_width(210.0);
                        ui.label(egui::RichText::new(key).monospace().small().color(p.accent));
                    });
                    ui.label(egui::RichText::new(what).small().color(p.text));
                });
            }
        });
    st.show_help = open;
}
