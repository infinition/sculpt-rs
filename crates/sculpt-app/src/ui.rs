//! egui side panel.

use crate::matcap;
use sculpt_core::{BrushKind, Sculptor};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Primitive {
    Sphere,
    Cube,
    Cylinder,
    Torus,
    Plane,
}

#[derive(Debug)]
pub enum Action {
    New(Primitive),
    Import,
    Export,
    Undo,
    Redo,
    ClearMask,
    InvertMask,
    FrameView,
    MatcapChanged,
}

pub struct UiState {
    pub matcap: matcap::Preset,
    pub wireframe: bool,
    pub wireframe_available: bool,
    pub show_mask: bool,
    pub vertex_color: bool,
    pub fps: f32,
    pub status: String,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            matcap: matcap::Preset::Clay,
            wireframe: false,
            wireframe_available: true,
            show_mask: true,
            vertex_color: true,
            fps: 0.0,
            status: String::new(),
        }
    }
}

/// Builds the panel. egui 0.36 hands the root closure a `Ui` rather than a
/// `Context`, and panels are shown inside it.
pub fn draw(
    root: &mut egui::Ui,
    s: &mut Sculptor,
    ui_state: &mut UiState,
    cursor: Option<(egui::Pos2, f32)>,
) -> Vec<Action> {
    let mut actions = Vec::new();

    egui::Panel::left("tools")
        .default_size(268.0)
        .show(root, |ui| {
            ui.add_space(4.0);
            ui.heading("sculpt-rs");
            ui.separator();

            ui.label(egui::RichText::new("BRUSH").small().strong());
            egui::Grid::new("brushes").num_columns(2).spacing([4.0, 4.0]).show(ui, |ui| {
                for (i, k) in BrushKind::ALL.iter().enumerate() {
                    if ui
                        .selectable_label(s.brush.kind == *k, k.label())
                        .clicked()
                    {
                        s.brush.kind = *k;
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
            ui.add_space(6.0);

            ui.add(egui::Slider::new(&mut s.brush.radius, 0.005..=1.0).logarithmic(true).text("Radius"));
            ui.add(egui::Slider::new(&mut s.brush.strength, 0.0..=1.0).text("Strength"));
            ui.checkbox(&mut s.brush.negative, "Invert (hold Ctrl)");

            if s.brush.kind == BrushKind::Paint {
                let mut rgb = [s.brush.paint_color.x, s.brush.paint_color.y, s.brush.paint_color.z];
                if ui.color_edit_button_rgb(&mut rgb).changed() {
                    s.brush.paint_color = glam::Vec3::from_array(rgb);
                }
            }

            ui.separator();
            ui.label(egui::RichText::new("TOPOLOGY").small().strong());
            ui.checkbox(&mut s.dyntopo_enabled, "Dynamic topology");
            ui.add_enabled_ui(s.dyntopo_enabled, |ui| {
                ui.add(
                    egui::Slider::new(&mut s.dyntopo.detail, 0.002..=0.3)
                        .logarithmic(true)
                        .text("Detail size"),
                );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut s.dyntopo.subdivide, "Subdivide");
                    ui.checkbox(&mut s.dyntopo.decimate, "Decimate");
                });
            });
            ui.checkbox(&mut s.symmetry, "Symmetry X  (X)");

            ui.separator();
            ui.label(egui::RichText::new("MASK").small().strong());
            ui.horizontal(|ui| {
                if ui.button("Clear").clicked() {
                    actions.push(Action::ClearMask);
                }
                if ui.button("Invert").clicked() {
                    actions.push(Action::InvertMask);
                }
                ui.checkbox(&mut ui_state.show_mask, "Show");
            });

            ui.separator();
            ui.label(egui::RichText::new("DISPLAY").small().strong());
            egui::ComboBox::from_label("Matcap")
                .selected_text(ui_state.matcap.label())
                .show_ui(ui, |ui| {
                    for p in matcap::Preset::ALL {
                        if ui.selectable_value(&mut ui_state.matcap, p, p.label()).clicked() {
                            actions.push(Action::MatcapChanged);
                        }
                    }
                });
            ui.checkbox(&mut ui_state.vertex_color, "Vertex colours");
            ui.add_enabled_ui(ui_state.wireframe_available, |ui| {
                ui.checkbox(&mut ui_state.wireframe, "Wireframe  (W)");
            });
            if !ui_state.wireframe_available {
                ui.label(
                    egui::RichText::new("wireframe needs POLYGON_MODE_LINE")
                        .small()
                        .weak(),
                );
            }
            if ui.button("Frame view  (F)").clicked() {
                actions.push(Action::FrameView);
            }

            ui.separator();
            ui.label(egui::RichText::new("SCENE").small().strong());
            egui::Grid::new("prims").num_columns(2).spacing([4.0, 4.0]).show(ui, |ui| {
                if ui.button("Sphere").clicked() {
                    actions.push(Action::New(Primitive::Sphere));
                }
                if ui.button("Cube").clicked() {
                    actions.push(Action::New(Primitive::Cube));
                }
                ui.end_row();
                if ui.button("Cylinder").clicked() {
                    actions.push(Action::New(Primitive::Cylinder));
                }
                if ui.button("Torus").clicked() {
                    actions.push(Action::New(Primitive::Torus));
                }
                ui.end_row();
                if ui.button("Plane").clicked() {
                    actions.push(Action::New(Primitive::Plane));
                }
                ui.end_row();
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Import").clicked() {
                    actions.push(Action::Import);
                }
                if ui.button("Export").clicked() {
                    actions.push(Action::Export);
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(s.history.can_undo(), egui::Button::new("Undo"))
                    .clicked()
                {
                    actions.push(Action::Undo);
                }
                if ui
                    .add_enabled(s.history.can_redo(), egui::Button::new("Redo"))
                    .clicked()
                {
                    actions.push(Action::Redo);
                }
            });

            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "{} verts   {} tris\n{:.0} fps   undo {:.0} MB",
                    s.mesh().vert_count(),
                    s.mesh().face_count(),
                    ui_state.fps,
                    s.history.used_bytes() as f64 / 1.0e6,
                ))
                .small()
                .monospace(),
            );
            if !ui_state.status.is_empty() {
                ui.label(egui::RichText::new(&ui_state.status).small().weak());
            }

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "LMB sculpt   MMB/RMB orbit\nShift+MMB pan   wheel zoom\nCtrl+Z undo",
                )
                .small()
                .weak(),
            );
        });

    // Brush cursor, painted over the viewport.
    if let Some((pos, radius_px)) = cursor {
        let painter = root.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("brush_cursor"),
        ));
        painter.circle_stroke(
            pos,
            radius_px,
            egui::Stroke::new(1.5, egui::Color32::from_white_alpha(190)),
        );
    }

    actions
}
