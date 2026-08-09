//! The interface: top bar, brush rail, contextual panel, viewport overlays.
//!
//! Layout is built around one idea: the tool you are holding is always visible
//! on the left, its settings are always in the same place on the right, and
//! everything you can touch is at least a fingertip wide. Nothing is buried in
//! a menu that a stylus has to hunt for.

use crate::camera::{Camera, Projection, ViewPreset};
use crate::icons::Icon;
use crate::matcap;
use crate::renderer::{FrameSettings, Shading};
use crate::theme::{Metrics, Palette};
use crate::widgets::{self, BigSlider};
use egui::{Align2, Color32, CornerRadius, Frame, Margin, Sense, Stroke, Vec2};
use sculpt_core::{Axis, BrushKind, Falloff, RemeshOptions, Sculptor};

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
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Brush,
    Model,
    Scene,
    View,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Brush, Tab::Model, Tab::Scene, Tab::View];

    fn label(self) -> &'static str {
        match self {
            Tab::Brush => "Brush",
            Tab::Model => "Model",
            Tab::Scene => "Scene",
            Tab::View => "View",
        }
    }
}

pub struct UiState {
    pub matcap: matcap::Preset,
    pub settings: FrameSettings,
    pub touch: bool,
    pub ui_scale: f32,
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
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            matcap: matcap::Preset::Clay,
            settings: FrameSettings::default(),
            touch: false,
            ui_scale: 1.0,
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

/// Builds the whole interface for one frame and returns the actions it raised.
pub fn draw(
    root: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    cam: &mut Camera,
    overlay: &Overlay,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let m = Metrics::for_touch(st.touch);

    top_bar(root, s, st, cam, &m, &mut actions);
    if st.show_rail {
        brush_rail(root, s, &m);
    }
    if st.show_panel {
        side_panel(root, s, st, cam, &m, &mut actions);
    }
    viewport_overlay(root, s, st, overlay, &mut actions);

    actions
}

// ---------------------------------------------------------------------------
// top bar
// ---------------------------------------------------------------------------

fn top_bar(
    root: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    cam: &mut Camera,
    m: &Metrics,
    actions: &mut Vec<Action>,
) {
    let height = m.button + 14.0;
    egui::Panel::top("topbar")
        .exact_size(height)
        .frame(
            Frame::new()
                .fill(Palette::PANEL)
                .inner_margin(Margin::symmetric(m.pad as i8, 6)),
        )
        .show(root, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(
                    egui::RichText::new("sculpt")
                        .strong()
                        .color(Palette::ACCENT)
                        .size(if st.touch { 18.0 } else { 15.0 }),
                );
                ui.add_space(m.gap);
                separator(ui, height);

                let b = m.button;
                if widgets::icon_button(ui, Icon::New, b, false, "New sphere").clicked() {
                    actions.push(Action::New(Primitive::Sphere));
                }
                if widgets::icon_button(ui, Icon::Open, b, false, "Open a mesh or scene").clicked() {
                    actions.push(Action::Import);
                }
                if widgets::icon_button(ui, Icon::Save, b, false, "Export the active mesh").clicked()
                {
                    actions.push(Action::Export);
                }
                separator(ui, height);

                let can_undo = s.history.can_undo();
                let can_redo = s.history.can_redo();
                ui.add_enabled_ui(can_undo, |ui| {
                    if widgets::icon_button(ui, Icon::Undo, b, false, "Undo   Ctrl+Z").clicked() {
                        actions.push(Action::Undo);
                    }
                });
                ui.add_enabled_ui(can_redo, |ui| {
                    if widgets::icon_button(ui, Icon::Redo, b, false, "Redo   Ctrl+Shift+Z")
                        .clicked()
                    {
                        actions.push(Action::Redo);
                    }
                });
                separator(ui, height);

                // Symmetry: one toggle plus the plane it works on.
                if widgets::icon_button(ui, Icon::Symmetry, b, s.symmetry, "Symmetry   X").clicked()
                {
                    s.symmetry = !s.symmetry;
                }
                let axis_labels = ["X", "Y", "Z"];
                let current = s.symmetry_axis.index();
                ui.scope(|ui| {
                    ui.set_width(if st.touch { 130.0 } else { 96.0 });
                    if let Some(i) = widgets::segmented(ui, &axis_labels, current, b * 0.72) {
                        s.symmetry_axis = Axis::ALL[i];
                        s.symmetry = true;
                    }
                });
                separator(ui, height);

                if widgets::icon_button(ui, Icon::Grid, b, st.settings.grid, "Ground grid").clicked()
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
                    actions.push(Action::FrameView);
                }
                if widgets::icon_button(
                    ui,
                    Icon::Camera,
                    b,
                    cam.projection == Projection::Orthographic,
                    "Orthographic view",
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
                        "Show or hide the panel   Tab",
                    )
                    .clicked()
                    {
                        st.show_panel = !st.show_panel;
                    }
                    if widgets::icon_button(ui, Icon::Settings, b, st.touch, "Touch layout")
                        .clicked()
                    {
                        st.touch = !st.touch;
                    }
                    if widgets::icon_button(ui, Icon::Pin, b, st.show_help, "Shortcuts   H").clicked()
                    {
                        st.show_help = !st.show_help;
                    }
                    ui.label(
                        egui::RichText::new(format!("{:>3.0} fps", st.fps))
                            .small()
                            .monospace()
                            .color(if st.fps < 25.0 { Palette::WARN } else { Palette::FAINT }),
                    );
                });
            });
        });
}

fn separator(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, height * 0.45), Sense::hover());
    ui.painter().line_segment(
        [
            egui::pos2(rect.center().x, rect.top()),
            egui::pos2(rect.center().x, rect.bottom()),
        ],
        Stroke::new(1.0, Palette::LINE),
    );
}

// ---------------------------------------------------------------------------
// brush rail
// ---------------------------------------------------------------------------

fn brush_rail(root: &mut egui::Ui, s: &mut Sculptor, m: &Metrics) {
    let width = m.tool + m.pad * 2.0;
    egui::Panel::left("rail")
        .exact_size(width)
        .frame(
            Frame::new()
                .fill(Palette::PANEL)
                .inner_margin(Margin::same(m.pad as i8)),
        )
        .show(root, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (i, kind) in BrushKind::ALL.iter().enumerate() {
                        let selected = s.brush.kind == *kind;
                        let tip = format!(
                            "{}\n{}\n\nShortcut: {}",
                            kind.label(),
                            kind.hint(),
                            shortcut_for(i)
                        );
                        if widgets::tool_tile(
                            ui,
                            Icon::of_brush(*kind),
                            kind.label(),
                            m.tool,
                            selected,
                            &tip,
                        )
                        .clicked()
                        {
                            s.set_brush_kind(*kind);
                        }
                    }
                });
        });
}

/// Keyboard shortcut shown for the nth tool.
pub fn shortcut_for(index: usize) -> String {
    match index {
        0..=8 => format!("{}", index + 1),
        9 => "0".into(),
        10 => "Shift+1".into(),
        11 => "Shift+2".into(),
        12 => "Shift+3".into(),
        _ => "-".into(),
    }
}

// ---------------------------------------------------------------------------
// side panel
// ---------------------------------------------------------------------------

fn side_panel(
    root: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    cam: &mut Camera,
    m: &Metrics,
    actions: &mut Vec<Action>,
) {
    let default = if st.touch { 344.0 } else { 300.0 };
    egui::Panel::right("panel")
        .default_size(default)
        .size_range(250.0..=480.0)
        .resizable(true)
        .frame(
            Frame::new()
                .fill(Palette::PANEL)
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
                    Tab::Brush => brush_tab(ui, s, st, m, actions),
                    Tab::Model => model_tab(ui, s, st, m, actions),
                    Tab::Scene => scene_tab(ui, s, st, m, actions),
                    Tab::View => view_tab(ui, st, cam, m, actions),
                });
        });
}

fn brush_tab(
    ui: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    m: &Metrics,
    actions: &mut Vec<Action>,
) {
    let kind = s.brush.kind;
    ui.label(
        egui::RichText::new(kind.label())
            .strong()
            .size(if st.touch { 17.0 } else { 15.0 })
            .color(Palette::TEXT),
    );
    ui.label(egui::RichText::new(kind.hint()).small().color(Palette::DIM));
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
    falloff_preview(ui, s.brush.falloff, m);

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

    if kind == BrushKind::Paint {
        widgets::section_title(ui, "PAINT");
        let mut rgb = [
            s.brush.paint_color.x,
            s.brush.paint_color.y,
            s.brush.paint_color.z,
        ];
        if widgets::color_row(ui, &mut rgb, m.row).changed() {
            s.brush.paint_color = glam::Vec3::from_array(rgb);
        }
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
        if widgets::wide_button(
            ui,
            Icon::Palette,
            "Pick a colour from the model",
            m.row,
            st.picking_color,
        )
        .clicked()
        {
            actions.push(Action::PickColorMode);
        }
    }

    if kind == BrushKind::Mask {
        widgets::section_title(ui, "MASK");
        mask_controls(ui, st, m, actions);
    }

    ui.add_space(8.0);
    if widgets::wide_button(ui, Icon::Reset, "Reset every tool", m.row, false).clicked() {
        actions.push(Action::ResetBrushes);
    }
}

/// Little curve showing what the selected falloff does.
fn falloff_preview(ui: &mut egui::Ui, falloff: Falloff, m: &Metrics) {
    let h = m.row * 1.3;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(9), Palette::BG);
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
    painter.add(egui::Shape::line(pts, Stroke::new(1.8, Palette::ACCENT)));
}

fn mask_controls(ui: &mut egui::Ui, st: &mut UiState, m: &Metrics, actions: &mut Vec<Action>) {
    let (clear, invert) = two_up(
        ui,
        m,
        |ui| button(ui, Icon::Close, "Clear", m),
        |ui| button(ui, Icon::Mirror, "Invert", m),
    );
    if clear {
        actions.push(Action::ClearMask);
    }
    if invert {
        actions.push(Action::InvertMask);
    }
    let (blur, sharpen) = two_up(
        ui,
        m,
        |ui| button(ui, Icon::Smooth, "Blur", m),
        |ui| button(ui, Icon::Crease, "Sharpen", m),
    );
    if blur {
        actions.push(Action::FilterMask(0.5));
    }
    if sharpen {
        actions.push(Action::FilterMask(-0.5));
    }
    widgets::toggle(ui, &mut st.settings.show_mask, "Show the mask", m.row);
    BigSlider::new(&mut st.extract_thickness, 0.005..=0.4, "Extract thickness")
        .logarithmic(true)
        .decimals(3)
        .height(m.row)
        .show(ui);
    if widgets::wide_button(ui, Icon::Duplicate, "Extract to a new object", m.row, false).clicked() {
        actions.push(Action::ExtractMask);
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
        let w = ((ui.available_width() - m.gap) * 0.5).max(40.0);
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

fn model_tab(
    ui: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    m: &Metrics,
    actions: &mut Vec<Action>,
) {
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
        actions.push(Action::Subdivide(st.subdivide_smooth));
    }
    BigSlider::new(&mut st.decimate_ratio, 0.05..=0.95, "Keep")
        .decimals(2)
        .height(m.row)
        .show(ui);
    if widgets::wide_button(ui, Icon::Decimate, "Decimate", m.row, false).clicked() {
        actions.push(Action::Decimate);
    }

    widgets::section_title(ui, "VOXEL REMESH");
    let mut res = st.remesh.resolution as f32;
    if BigSlider::new(&mut res, 24.0..=350.0, "Resolution")
        .decimals(0)
        .height(m.row)
        .show(ui)
        .changed()
    {
        st.remesh.resolution = res as u32;
    }
    let mut smooth = st.remesh.smoothing as f32;
    if BigSlider::new(&mut smooth, 0.0..=6.0, "Smoothing passes")
        .decimals(0)
        .height(m.row)
        .show(ui)
        .changed()
    {
        st.remesh.smoothing = smooth as u32;
    }
    widgets::toggle(ui, &mut st.remesh.transfer_colors, "Keep colours", m.row);
    if widgets::wide_button(ui, Icon::Remesh, "Remesh", m.row, false).clicked() {
        actions.push(Action::Remesh);
    }

    widgets::section_title(ui, "REPAIR");
    if widgets::wide_button(ui, Icon::CloseHoles, "Close holes", m.row, false).clicked() {
        actions.push(Action::CloseHoles);
    }
    if widgets::wide_button(ui, Icon::Smooth, "Relax the whole mesh", m.row, false).clicked() {
        actions.push(Action::SmoothAll);
    }

    widgets::section_title(ui, "SYMMETRY TOOLS");
    ui.horizontal(|ui| {
        let w = (ui.available_width() - m.gap * 2.0) / 3.0;
        for axis in Axis::ALL {
            ui.scope(|ui| {
                ui.set_width(w.max(40.0));
                if widgets::wide_button(ui, Icon::Mirror, axis.label(), m.row, false).clicked() {
                    actions.push(Action::Mirror(axis));
                }
            });
        }
    });
    ui.label(
        egui::RichText::new("Symmetrize replaces one half with the mirror of the other.")
            .small()
            .color(Palette::DIM),
    );
    let axis = s.symmetry_axis;
    let (keep_plus, keep_minus) = two_up(
        ui,
        m,
        |ui| button(ui, Icon::Symmetry, "Keep +", m),
        |ui| button(ui, Icon::Symmetry, "Keep -", m),
    );
    if keep_plus {
        actions.push(Action::Symmetrize(axis, true));
    }
    if keep_minus {
        actions.push(Action::Symmetrize(axis, false));
    }

    widgets::section_title(ui, "MASK");
    mask_controls(ui, st, m, actions);
}

fn scene_tab(
    ui: &mut egui::Ui,
    s: &mut Sculptor,
    st: &mut UiState,
    m: &Metrics,
    actions: &mut Vec<Action>,
) {
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
                actions.push(Action::ToggleVisible(i));
            }
            let remaining = ui.available_width() - (m.row + m.gap) * 2.0;
            ui.scope(|ui| {
                ui.set_width(remaining.max(60.0));
                if widgets::wide_button(ui, Icon::Remesh, &name, m.row, selected).clicked() {
                    actions.push(Action::SelectObject(i));
                }
            });
            if widgets::icon_button(ui, Icon::Duplicate, m.row, false, "Duplicate").clicked() {
                actions.push(Action::DuplicateObject(i));
            }
            ui.add_enabled_ui(count > 1, |ui| {
                if widgets::icon_button(ui, Icon::Trash, m.row, false, "Delete").clicked() {
                    actions.push(Action::DeleteObject(i));
                }
            });
        });
    }

    ui.add_space(4.0);
    ui.add_enabled_ui(count > 1, |ui| {
        if widgets::wide_button(ui, Icon::Remesh, "Merge everything visible", m.row, false).clicked()
        {
            actions.push(Action::MergeVisible);
        }
    });

    widgets::section_title(ui, "ADD");
    let current = Primitive::ALL
        .iter()
        .position(|p| *p == st.add_primitive)
        .unwrap_or(0);
    egui::ComboBox::from_id_salt("prim")
        .selected_text(Primitive::ALL[current].label())
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for p in Primitive::ALL {
                ui.selectable_value(&mut st.add_primitive, p, p.label());
            }
        });
    let prim = st.add_primitive;
    let (add, replace) = two_up(
        ui,
        m,
        |ui| button(ui, Icon::Plus, "Add", m),
        |ui| button(ui, Icon::New, "Replace", m),
    );
    if add {
        actions.push(Action::AddObject(prim));
    }
    if replace {
        actions.push(Action::New(prim));
    }

    widgets::section_title(ui, "PLACEMENT");
    if let Some(o) = s.scene.active_mut() {
        let mut p = o.transform.position;
        let mut moved = false;
        moved |= BigSlider::new(&mut p.x, -3.0..=3.0, "X")
            .decimals(3)
            .height(m.row)
            .show(ui)
            .changed();
        moved |= BigSlider::new(&mut p.y, -3.0..=3.0, "Y")
            .decimals(3)
            .height(m.row)
            .show(ui)
            .changed();
        moved |= BigSlider::new(&mut p.z, -3.0..=3.0, "Z")
            .decimals(3)
            .height(m.row)
            .show(ui)
            .changed();
        let mut scale = o.transform.scale.x;
        let scaled = BigSlider::new(&mut scale, 0.05..=6.0, "Scale")
            .logarithmic(true)
            .decimals(3)
            .height(m.row)
            .show(ui)
            .changed();
        if moved {
            o.transform.position = p;
        }
        if scaled {
            o.transform.scale = glam::Vec3::splat(scale);
        }
    }
    if widgets::wide_button(ui, Icon::Pin, "Bake the placement in", m.row, false).clicked() {
        actions.push(Action::ApplyTransform);
    }

    widgets::section_title(ui, "FILES");
    if widgets::wide_button(ui, Icon::Open, "Import a mesh", m.row, false).clicked() {
        actions.push(Action::Import);
    }
    if widgets::wide_button(ui, Icon::Save, "Export the active mesh", m.row, false).clicked() {
        actions.push(Action::Export);
    }
    if widgets::wide_button(ui, Icon::Open, "Open a scene", m.row, false).clicked() {
        actions.push(Action::OpenScene);
    }
    if widgets::wide_button(ui, Icon::Save, "Save the scene", m.row, false).clicked() {
        actions.push(Action::SaveScene);
    }
    ui.label(
        egui::RichText::new("OBJ, PLY and STL for meshes; .sculpt keeps the whole scene.")
            .small()
            .color(Palette::DIM),
    );
}

fn view_tab(
    ui: &mut egui::Ui,
    st: &mut UiState,
    cam: &mut Camera,
    m: &Metrics,
    actions: &mut Vec<Action>,
) {
    widgets::section_title(ui, "SHADING");
    let labels: Vec<&str> = Shading::ALL.iter().map(|s| s.label()).collect();
    let current = Shading::ALL
        .iter()
        .position(|s| *s == st.settings.shading)
        .unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
        st.settings.shading = Shading::ALL[i];
    }

    if matches!(st.settings.shading, Shading::Matcap | Shading::Cavity) {
        egui::ComboBox::from_id_salt("matcap")
            .selected_text(st.matcap.label())
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for p in matcap::Preset::ALL {
                    if ui.selectable_value(&mut st.matcap, p, p.label()).clicked() {
                        actions.push(Action::MatcapChanged);
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
                .color(Palette::DIM),
        );
    }
    BigSlider::new(&mut st.settings.opacity, 0.15..=1.0, "Opacity")
        .decimals(2)
        .height(m.row)
        .show(ui);

    widgets::section_title(ui, "SCENE");
    widgets::toggle(ui, &mut st.settings.grid, "Ground grid", m.row);
    let grid = st.settings.grid;
    ui.add_enabled_ui(grid, |ui| {
        BigSlider::new(&mut st.settings.grid_spacing, 0.02..=2.0, "Grid spacing")
            .logarithmic(true)
            .decimals(3)
            .height(m.row)
            .show(ui);
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Background").small().color(Palette::DIM));
        ui.color_edit_button_rgb(&mut st.settings.background_top);
        ui.color_edit_button_rgb(&mut st.settings.background_bottom);
    });

    widgets::section_title(ui, "CAMERA");
    let proj_labels = ["Perspective", "Ortho"];
    let proj_current = if cam.projection == Projection::Perspective { 0 } else { 1 };
    if let Some(i) = widgets::segmented(ui, &proj_labels, proj_current, m.row) {
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
    ui.horizontal_wrapped(|ui| {
        for preset in ViewPreset::ALL {
            ui.scope(|ui| {
                ui.set_width(if st.touch { 100.0 } else { 84.0 });
                if widgets::wide_button(ui, Icon::Camera, preset.label(), m.row, false).clicked() {
                    actions.push(Action::SetView(preset));
                }
            });
        }
    });
    if widgets::wide_button(ui, Icon::FrameView, "Frame the model   F", m.row, false).clicked() {
        actions.push(Action::FrameView);
    }

    widgets::section_title(ui, "INTERFACE");
    widgets::toggle(ui, &mut st.touch, "Touch layout", m.row);
    widgets::toggle(ui, &mut st.show_rail, "Show the tool rail", m.row);
    widgets::toggle(ui, &mut st.show_stats, "Show statistics", m.row);
    BigSlider::new(&mut st.ui_scale, 0.7..=2.0, "Interface scale")
        .decimals(2)
        .height(m.row)
        .show(ui);

    widgets::section_title(ui, "ANTI-ALIASING");
    let msaa_ok = st.msaa_available;
    ui.add_enabled_ui(msaa_ok, |ui| {
        let labels = ["Off", "2x", "4x", "8x"];
        let counts = [1u32, 2, 4, 8];
        let current = counts.iter().position(|c| *c == st.sample_count).unwrap_or(2);
        if let Some(i) = widgets::segmented(ui, &labels, current, m.row) {
            st.sample_count = counts[i];
            actions.push(Action::SampleCountChanged(counts[i]));
        }
    });
}

// ---------------------------------------------------------------------------
// viewport overlays
// ---------------------------------------------------------------------------

fn viewport_overlay(
    root: &mut egui::Ui,
    s: &Sculptor,
    st: &mut UiState,
    overlay: &Overlay,
    actions: &mut Vec<Action>,
) {
    let ctx = root.ctx().clone();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("viewport_overlay"),
    ));

    // Brush cursor: outer ring is the radius, inner ring the strength.
    if let Some((pos, radius)) = overlay.cursor {
        let accent = if overlay.stroking {
            Palette::ACCENT
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
            "{} verts   {} tris\nundo {:.0} MB   {} objects",
            s.mesh().vert_count(),
            s.mesh().face_count(),
            s.history.used_bytes() as f64 / 1.0e6,
            s.scene.objects.len(),
        );
        painter.text(
            egui::pos2(rect.left() + 12.0, rect.bottom() - 12.0),
            Align2::LEFT_BOTTOM,
            text,
            egui::FontId::monospace(11.0),
            Palette::FAINT,
        );
    }

    // Transient status message, bottom centre.
    if !st.status.is_empty() && st.status_age < 4.0 {
        let alpha = (4.0 - st.status_age).clamp(0.0, 1.0);
        let color = Palette::TEXT.gamma_multiply(alpha);
        let galley =
            painter.layout_no_wrap(st.status.clone(), egui::FontId::proportional(13.0), color);
        let bg = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.bottom() - 26.0),
            galley.size() + Vec2::new(22.0, 12.0),
        );
        painter.rect_filled(
            bg,
            CornerRadius::same(10),
            Palette::RAISED.gamma_multiply(alpha * 0.95),
        );
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
        help_window(&ctx, st);
    }
    let _ = actions;
}

fn help_window(ctx: &egui::Context, st: &mut UiState) {
    let mut open = st.show_help;
    egui::Window::new("Shortcuts")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .frame(
            Frame::window(&ctx.style_of(egui::Theme::Dark))
                .fill(Palette::PANEL)
                .stroke(Stroke::new(1.0, Palette::LINE)),
        )
        .show(ctx, |ui| {
            let rows: [(&str, &str); 15] = [
                ("Left mouse / one finger", "Sculpt"),
                ("Middle or right mouse", "Orbit"),
                ("Two fingers", "Orbit, pan and pinch to zoom"),
                ("Shift + middle mouse", "Pan"),
                ("Wheel", "Zoom"),
                ("Alt + left mouse", "Orbit without sculpting"),
                ("Ctrl (hold)", "Invert the brush"),
                ("Shift (hold)", "Smooth instead of sculpting"),
                ("[ and ]", "Brush radius"),
                ("- and =", "Brush strength"),
                ("1 to 0, Shift+1..3", "Pick a tool"),
                ("X / W / F", "Symmetry, wireframe, frame the model"),
                ("Tab", "Show or hide the panel"),
                ("Ctrl+Z / Ctrl+Shift+Z", "Undo and redo"),
                ("Numpad 1 / 3 / 7", "Front, right and top views"),
            ];
            for (key, what) in rows {
                ui.horizontal(|ui| {
                    ui.scope(|ui| {
                        ui.set_width(200.0);
                        ui.label(
                            egui::RichText::new(key)
                                .monospace()
                                .small()
                                .color(Palette::ACCENT),
                        );
                    });
                    ui.label(egui::RichText::new(what).small().color(Palette::TEXT));
                });
            }
        });
    st.show_help = open;
}
