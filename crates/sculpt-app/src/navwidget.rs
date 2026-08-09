//! Orientation widget: a live axis ball in the corner of the viewport.
//!
//! It answers "which way am I looking" at a glance and lets you fix it without
//! reaching for a menu. The six axis balls are projected through the camera
//! every frame, sorted back to front so the near ones cover the far ones, and
//! any of them can be clicked to swing round to that view. Holding the widget
//! down locks the view, which is what you want when a stroke keeps nudging the
//! camera.

use crate::camera::{Camera, Projection, ViewPreset};
use crate::icons::Icon;
use crate::theme::{Metrics, Palette};
use crate::widgets;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use glam::Vec3;

/// How long the widget has to be held before it toggles the view lock.
const HOLD_SECONDS: f64 = 0.4;

#[derive(Clone, Copy, Debug)]
pub enum NavAction {
    /// Swing round to a named view.
    View(ViewPreset),
    /// Snap to whichever named view is closest to the current one.
    Align,
    Frame,
    ToggleProjection,
    ToggleLock,
    /// Drag on the ball, in points, to be spun into an orbit.
    Orbit(Vec2),
}

/// How far the pointer may wander before a press counts as a drag rather than
/// a click. Small enough that a deliberate spin registers at once, large enough
/// that a tap on an axis is not stolen by a shaking hand.
const DRAG_SLOP: f32 = 3.0;
/// Dragging the ball turns the model a little faster than dragging the
/// viewport: the ball is small, and the wrist travel available is short.
const BALL_ORBIT_GAIN: f32 = 1.6;

pub struct NavWidget {
    /// Diameter of the ball, in points.
    pub size: f32,
    pub show_buttons: bool,
    /// When the current press began, for the hold-to-lock gesture.
    pressed_at: Option<f64>,
    /// Ball the press started on, if any.
    pressed_axis: Option<usize>,
    /// Set once the press has travelled far enough to be a spin, which rules
    /// out both the tap and the hold.
    spinning: bool,
}

impl Default for NavWidget {
    fn default() -> Self {
        Self {
            size: 104.0,
            show_buttons: true,
            pressed_at: None,
            pressed_axis: None,
            spinning: false,
        }
    }
}

/// The six directions, in the order they are drawn and hit-tested.
///
/// A view is named for where the camera sits, so looking from +Z is the front.
const AXES: [(Vec3, ViewPreset, &str, usize); 6] = [
    (Vec3::Z, ViewPreset::Front, "F", 2),
    (Vec3::NEG_Z, ViewPreset::Back, "B", 2),
    (Vec3::X, ViewPreset::Right, "R", 0),
    (Vec3::NEG_X, ViewPreset::Left, "L", 0),
    (Vec3::Y, ViewPreset::Top, "T", 1),
    (Vec3::NEG_Y, ViewPreset::Bottom, "U", 1),
];

fn axis_color(index: usize) -> Color32 {
    match index {
        0 => Color32::from_rgb(232, 84, 96),
        1 => Color32::from_rgb(120, 214, 108),
        _ => Color32::from_rgb(88, 150, 246),
    }
}

/// The named view closest to where the camera currently sits.
pub fn nearest_view(camera: &Camera) -> (ViewPreset, f32) {
    let dir = (camera.eye() - camera.target()).normalize_or(Vec3::Z);
    let mut best = (ViewPreset::Front, -2.0);
    for (axis, preset, _, _) in AXES {
        let d = dir.dot(axis);
        if d > best.1 {
            best = (preset, d);
        }
    }
    best
}

impl NavWidget {
    /// Draws the widget at the top right of `viewport` and reports what was
    /// asked of it.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        viewport: Rect,
        camera: &Camera,
        p: &Palette,
        m: &Metrics,
    ) -> Option<NavAction> {
        let pad = 14.0;
        let ball = self.size;
        let buttons = if self.show_buttons { m.button + 6.0 } else { 0.0 };
        let width = ball.max(m.button * 4.0 + 12.0);
        // Buttons, ball, then the name plate under it.
        let badge = 24.0;
        let area = Rect::from_min_size(
            Pos2::new(viewport.right() - width - pad, viewport.top() + pad),
            Vec2::new(width, ball + buttons + badge),
        );
        // Nothing to draw if the viewport is too small to hold it.
        if area.width() > viewport.width() || area.height() > viewport.height() {
            return None;
        }

        let mut action = None;
        egui::Area::new(egui::Id::new("nav_widget"))
            .fixed_pos(area.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_width(area.width());
                // The buttons sit above the ball: they are the things you reach
                // for on purpose, and putting them at the top keeps them clear
                // of whatever else floats in that corner.
                if self.show_buttons {
                    let row = Rect::from_min_size(
                        area.min,
                        Vec2::new(area.width(), m.button),
                    );
                    action = self.draw_buttons(ui, row, camera, m);
                }
                let ball_top = if self.show_buttons { area.top() + buttons } else { area.top() };
                let ball_rect = Rect::from_center_size(
                    Pos2::new(area.center().x, ball_top + ball * 0.5),
                    Vec2::splat(ball),
                );
                if let Some(a) = self.draw_ball(ui, ball_rect, camera, p) {
                    action = Some(a);
                }
                self.draw_badge(ui, ball_rect, camera, p);
            });
        action
    }

    fn draw_ball(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        camera: &Camera,
        p: &Palette,
    ) -> Option<NavAction> {
        let response = ui.interact(rect, ui.id().with("ball"), Sense::click_and_drag());
        let painter = ui.painter();
        let center = rect.center();
        let radius = rect.width() * 0.5;

        let hovered = response.hovered();
        painter.circle(
            center,
            radius,
            p.panel.gamma_multiply(if hovered { 0.95 } else { 0.75 }),
            Stroke::new(1.0, if camera.locked { p.accent } else { p.line }),
        );

        // Project every axis through the camera's rotation only: the widget
        // shows orientation, not position.
        let view = camera.view();
        let mut balls: Vec<(f32, Pos2, usize, bool, &str, ViewPreset)> = AXES
            .iter()
            .map(|(axis, preset, label, colour)| {
                let v = view.transform_vector3(*axis);
                let pos = Pos2::new(
                    center.x + v.x * radius * 0.68,
                    center.y - v.y * radius * 0.68,
                );
                // In a right-handed view space the camera looks down -Z, so a
                // larger z is nearer.
                (v.z, pos, *colour, axis.max_element() > 0.0, *label, *preset)
            })
            .collect();
        balls.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Spokes first, so the balls sit on top of them.
        for (_, pos, colour, positive, _, _) in &balls {
            if *positive {
                painter.line_segment(
                    [center, *pos],
                    Stroke::new(2.0, axis_color(*colour).gamma_multiply(0.55)),
                );
            }
        }

        let cursor = ui.ctx().pointer_latest_pos();
        let dot = radius * 0.24;
        let mut hot: Option<usize> = None;
        for (i, (depth, pos, colour, positive, label, _)) in balls.iter().enumerate() {
            let near = (*depth + 1.0) * 0.5;
            let r = dot * if *positive { 1.0 } else { 0.82 };
            let over = cursor.is_some_and(|c| (c - *pos).length() < r + 3.0);
            if over {
                hot = Some(i);
            }
            let base = axis_color(*colour);
            let fill = if *positive {
                base.gamma_multiply(0.55 + 0.45 * near)
            } else {
                p.panel.gamma_multiply(0.9)
            };
            painter.circle(
                *pos,
                if over { r * 1.18 } else { r },
                fill,
                Stroke::new(if *positive { 0.0 } else { 1.5 }, base.gamma_multiply(0.8)),
            );
            // Only the three positive axes are named. Lettering the far side as
            // well, even on hover, turns the ball into an eye chart, and the
            // colour already says which axis it is.
            if *positive {
                painter.text(
                    *pos,
                    Align2::CENTER_CENTER,
                    *label,
                    FontId::proportional(r * 1.1),
                    p.bg,
                );
            }
        }


        // One press, three possible meanings, resolved by what happens next:
        // move and it spins the model, wait and it locks the view, do neither
        // and let go on an axis to swing round to it.
        let now = ui.input(|i| i.time);
        if response.is_pointer_button_down_on() && self.pressed_at.is_none() {
            self.pressed_at = Some(now);
            self.pressed_axis = hot;
            self.spinning = false;
        }

        let mut action = None;
        if let Some(started) = self.pressed_at {
            let travel = response.drag_delta();
            if !self.spinning && travel.length() > DRAG_SLOP {
                self.spinning = true;
            }
            if self.spinning {
                if travel != Vec2::ZERO {
                    action = Some(NavAction::Orbit(travel * BALL_ORBIT_GAIN));
                }
                painter.circle_stroke(
                    center,
                    radius - 2.0,
                    Stroke::new(1.5, p.accent.gamma_multiply(0.6)),
                );
            } else {
                let held = now - started;
                if held > HOLD_SECONDS {
                    // Show the lock arming as a ring closing around the ball.
                    painter.circle_stroke(center, radius - 2.0, Stroke::new(2.0, p.accent));
                }
            }

            if !response.is_pointer_button_down_on() {
                if !self.spinning {
                    let held = now - started;
                    if held > HOLD_SECONDS {
                        action = Some(NavAction::ToggleLock);
                    } else if let Some(i) = self.pressed_axis {
                        action = Some(NavAction::View(balls[i].5));
                    }
                }
                self.pressed_at = None;
                self.pressed_axis = None;
                self.spinning = false;
            }
        }
        if response.hovered() && action.is_none() {
            response.on_hover_text(
                "Drag to spin the view.\nClick an axis to swing round to it.\nHold to lock.",
            );
        }
        action
    }

    /// The name of the view we are closest to, in a small plate under the ball.
    ///
    /// It used to sit inside the ball, where it crossed the axes and whatever
    /// the ball was drawn over. Underneath it reads cleanly and never covers
    /// anything.
    fn draw_badge(&self, ui: &egui::Ui, ball: Rect, camera: &Camera, p: &Palette) {
        let (preset, alignment) = nearest_view(camera);
        let aligned = alignment > 0.995;
        let text = if camera.locked {
            format!("{}  ·  locked", preset.label())
        } else {
            preset.label().to_string()
        };
        let painter = ui.painter();
        let galley = painter.layout_no_wrap(text, FontId::proportional(10.5), p.dim);
        let plate = Rect::from_center_size(
            Pos2::new(ball.center().x, ball.bottom() + 11.0),
            galley.size() + Vec2::new(14.0, 6.0),
        );
        let colour = if camera.locked || aligned { p.accent } else { p.dim };
        painter.rect(
            plate,
            egui::CornerRadius::same((plate.height() * 0.5) as u8),
            p.panel.gamma_multiply(0.92),
            Stroke::new(1.0, if camera.locked || aligned { colour } else { p.line }),
            egui::StrokeKind::Inside,
        );
        painter.galley(
            Pos2::new(
                plate.center().x - galley.size().x * 0.5,
                plate.center().y - galley.size().y * 0.5,
            ),
            galley,
            colour,
        );
    }

    fn draw_buttons(
        &mut self,
        ui: &mut egui::Ui,
        row: Rect,
        camera: &Camera,
        m: &Metrics,
    ) -> Option<NavAction> {
        let mut action = None;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(row));
        child.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let b = m.button;
            if widgets::icon_button(
                ui,
                if camera.locked { Icon::Lock } else { Icon::Unlock },
                b,
                camera.locked,
                "Lock the view",
            )
            .clicked()
            {
                action = Some(NavAction::ToggleLock);
            }
            if widgets::icon_button(ui, Icon::FrameView, b, false, "Frame the model   F").clicked() {
                action = Some(NavAction::Frame);
            }
            if widgets::icon_button(ui, Icon::Align, b, false, "Snap to the nearest view").clicked()
            {
                action = Some(NavAction::Align);
            }
            if widgets::icon_button(
                ui,
                Icon::Camera,
                b,
                camera.projection == Projection::Orthographic,
                "Perspective or orthographic   Numpad 5",
            )
            .clicked()
            {
                action = Some(NavAction::ToggleProjection);
            }
        });
        action
    }
}
