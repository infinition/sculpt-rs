//! Floating controls: the things a hand on a tablet needs within reach.
//!
//! Two clusters. Beside the tool rail, a tall pill for size and one for force
//! with a round button between them that opens the radial menu when held, and
//! shows either the tool in your hand or, when painting, the colour. Under the
//! orientation ball, three round buttons that orbit, pan and zoom on a drag,
//! because a tablet has no middle mouse button and no wheel.
//!
//! Both clusters float over the viewport at a position stored as a fraction of
//! it, so they stay put when the window resizes, and either can be dragged
//! somewhere else once Arrange is on.

use crate::icons::{self, Icon};
use crate::theme::Palette;
#[allow(unused_imports)]
use crate::widgets;
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Vec2};
use sculpt_core::Sculptor;

/// How long the round button is held before the radial menu appears.
const HOLD_SECONDS: f64 = 0.28;
/// Points of vertical drag for a pill to travel its whole range.
const PILL_TRAVEL: f32 = 220.0;

#[derive(Clone, Copy, Debug)]
pub enum HudAction {
    /// Open the radial menu. Where it goes is the caller's business: the
    /// button that raised this sits at the edge of the screen, which is the
    /// one place a ring should not be centred on.
    OpenWheel,
    Orbit(Vec2),
    Pan(Vec2),
    Zoom(f32),
}

/// Which cluster, for the drag-to-arrange code.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cluster {
    Brush,
    Nav,
}

pub struct Hud {
    pub show_brush: bool,
    pub show_nav: bool,
    /// Drag the clusters to move them instead of using them.
    pub arrange: bool,
    /// Size of a round button, in points. The pills scale from it.
    pub size: f32,
    /// Centres, as a fraction of the viewport.
    pub brush_at: Vec2,
    pub nav_at: Vec2,
    /// When the round button went down, for the hold gesture.
    pressed_at: Option<f64>,
    wheel_open: bool,
    dragging: Option<Cluster>,
    /// True while a size or force pill is being dragged, so the viewport can
    /// show what the brush now looks like.
    adjusting: bool,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            show_brush: true,
            show_nav: true,
            arrange: false,
            size: 52.0,
            // Just clear of the tool rail, and under the orientation ball.
            brush_at: Vec2::new(0.07, 0.52),
            nav_at: Vec2::new(0.945, 0.30),
            pressed_at: None,
            wheel_open: false,
            dragging: None,
            adjusting: false,
        }
    }
}

impl Hud {
    pub fn reset_positions(&mut self) {
        let d = Hud::default();
        self.brush_at = d.brush_at;
        self.nav_at = d.nav_at;
    }

    /// Draws both clusters and reports what they were asked to do.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        viewport: Rect,
        s: &mut Sculptor,
        p: &Palette,
        wheel_open: bool,
    ) -> Vec<HudAction> {
        let mut actions = Vec::new();
        self.adjusting = false;
        self.wheel_open = wheel_open;
        if self.show_brush {
            self.brush_cluster(ctx, viewport, s, p, &mut actions);
        }
        if self.show_nav {
            self.nav_cluster(ctx, viewport, p, &mut actions);
        }
        actions
    }

    /// True while size or force is being dragged.
    pub fn is_adjusting(&self) -> bool {
        self.adjusting
    }

    fn place(&self, viewport: Rect, at: Vec2, size: Vec2) -> Rect {
        let centre = egui::pos2(
            viewport.left() + viewport.width() * at.x,
            viewport.top() + viewport.height() * at.y,
        );
        // Never let a cluster leave the viewport, whatever the window does.
        let half = size * 0.5;
        let clamped = egui::pos2(
            centre
                .x
                .clamp(viewport.left() + half.x, (viewport.right() - half.x).max(viewport.left() + half.x)),
            centre
                .y
                .clamp(viewport.top() + half.y, (viewport.bottom() - half.y).max(viewport.top() + half.y)),
        );
        Rect::from_center_size(clamped, size)
    }

    /// Handles the drag that moves a cluster while Arrange is on.
    fn arrange_drag(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        viewport: Rect,
        cluster: Cluster,
        p: &Palette,
    ) -> bool {
        if !self.arrange {
            return false;
        }
        let id = ui.id().with(("arrange", cluster == Cluster::Brush));
        let response = ui.interact(rect, id, Sense::click_and_drag());
        ui.painter().rect(
            rect.expand(3.0),
            CornerRadius::same(14),
            Color32::TRANSPARENT,
            Stroke::new(1.5, p.accent.gamma_multiply(if response.hovered() { 1.0 } else { 0.5 })),
            egui::StrokeKind::Inside,
        );
        if response.dragged() {
            let d = response.drag_delta();
            let at = match cluster {
                Cluster::Brush => &mut self.brush_at,
                Cluster::Nav => &mut self.nav_at,
            };
            at.x = (at.x + d.x / viewport.width().max(1.0)).clamp(0.0, 1.0);
            at.y = (at.y + d.y / viewport.height().max(1.0)).clamp(0.0, 1.0);
            self.dragging = Some(cluster);
        } else if self.dragging == Some(cluster) && !response.dragged() {
            self.dragging = None;
        }
        true
    }

    // ---- size, force and the menu button ------------------------------------

    fn brush_cluster(
        &mut self,
        ctx: &egui::Context,
        viewport: Rect,
        s: &mut Sculptor,
        p: &Palette,
        actions: &mut Vec<HudAction>,
    ) {
        let round = self.size;
        let pill = Vec2::new(round * 0.72, round * 2.6);
        let gap = 8.0;
        let total = Vec2::new(
            pill.x * 2.0 + round + gap * 2.0,
            pill.y.max(round),
        );
        let rect = self.place(viewport, self.brush_at, total);

        egui::Area::new(egui::Id::new("hud_brush"))
            .fixed_pos(rect.min)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                ui.set_min_size(total);
                if self.arrange_drag(ui, rect, viewport, Cluster::Brush, p) {
                    // While arranging, the controls are scenery.
                    self.draw_pill(ui, left_rect(rect, pill), "Size", s.brush.radius, 0.003, 1.5, true, p);
                    self.draw_pill(
                        ui,
                        right_rect(rect, pill),
                        "Force",
                        s.brush.strength,
                        0.0,
                        1.0,
                        false,
                        p,
                    );
                    self.draw_round(ui, centre_rect(rect, round), s, p, false);
                    return;
                }

                // Size travels geometrically, force linearly.
                let left = left_rect(rect, pill);
                if let Some(t) = self.pill_drag(ui, left, "size") {
                    s.brush.radius = (s.brush.radius * 8f32.powf(t)).clamp(0.003, 1.5);
                    self.adjusting = true;
                }
                self.draw_pill(ui, left, "Size", s.brush.radius, 0.003, 1.5, true, p);

                let right = right_rect(rect, pill);
                if let Some(t) = self.pill_drag(ui, right, "force") {
                    s.brush.strength = (s.brush.strength + t).clamp(0.0, 1.0);
                    self.adjusting = true;
                }
                self.draw_pill(ui, right, "Force", s.brush.strength, 0.0, 1.0, false, p);

                let centre = centre_rect(rect, round);
                let response = ui.interact(centre, ui.id().with("menu"), Sense::click_and_drag());
                let now = ui.input(|i| i.time);

                // This button only ever opens the menu. Closing it belongs to
                // the menu, which is the thing that knows whether the finger
                // that opened it has lifted; splitting that decision across two
                // places is what had it flickering shut.
                if self.wheel_open {
                    self.pressed_at = None;
                } else if response.is_pointer_button_down_on() {
                    let started = *self.pressed_at.get_or_insert(now);
                    if now - started > HOLD_SECONDS {
                        actions.push(HudAction::OpenWheel);
                    }
                } else {
                    self.pressed_at = None;
                }
                self.draw_round(ui, centre, s, p, response.hovered());
            });
    }

    /// A vertical drag on a pill, returned as a fraction of its full travel.
    ///
    /// Reports zero rather than nothing while the finger rests, so the caller
    /// can tell "holding still" from "not holding at all": the preview has to
    /// stay up through a pause, not blink out the moment the hand stops.
    fn pill_drag(&self, ui: &egui::Ui, rect: Rect, salt: &str) -> Option<f32> {
        let response = ui.interact(rect, ui.id().with(salt), Sense::click_and_drag());
        if !response.dragged() && !response.is_pointer_button_down_on() {
            return None;
        }
        // Up increases, which is the way every physical fader works.
        Some(-response.drag_delta().y / PILL_TRAVEL)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_pill(
        &self,
        ui: &egui::Ui,
        rect: Rect,
        label: &str,
        value: f32,
        lo: f32,
        hi: f32,
        logarithmic: bool,
        p: &Palette,
    ) {
        let painter = ui.painter();
        let radius = CornerRadius::same((rect.width() * 0.5) as u8);
        painter.rect_filled(rect, radius, p.panel.gamma_multiply(0.92));
        painter.rect_stroke(rect, radius, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);

        let t = if logarithmic {
            ((value.max(1e-6) / lo.max(1e-6)).ln() / (hi / lo.max(1e-6)).ln()).clamp(0.0, 1.0)
        } else {
            ((value - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0)
        };
        let filled = Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - rect.height() * t),
            rect.max,
        );
        if filled.height() > 2.0 {
            painter.rect_filled(filled, radius, p.accent_dim);
        }

        painter.text(
            egui::pos2(rect.center().x, rect.top() + 11.0),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(9.5),
            p.dim,
        );
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() - 11.0),
            Align2::CENTER_CENTER,
            if logarithmic {
                format!("{value:.3}")
            } else {
                format!("{value:.2}")
            },
            FontId::proportional(9.5),
            p.text,
        );
    }

    /// The round button between the pills: the tool, or the colour when
    /// painting, and the way into the radial menu.
    fn draw_round(&self, ui: &egui::Ui, rect: Rect, s: &Sculptor, p: &Palette, hovered: bool) {
        let painter = ui.painter();
        let r = rect.width() * 0.5;
        let painting = s.brush.kind.paints();
        let fill = if painting {
            let c = s.brush.paint_color;
            Color32::from_rgb(
                (c.x.clamp(0.0, 1.0) * 255.0) as u8,
                (c.y.clamp(0.0, 1.0) * 255.0) as u8,
                (c.z.clamp(0.0, 1.0) * 255.0) as u8,
            )
        } else {
            p.raised
        };
        painter.circle(
            rect.center(),
            r,
            fill,
            Stroke::new(if hovered { 2.0 } else { 1.2 }, if hovered { p.accent } else { p.line }),
        );

        // A held button fills its ring as the menu arms.
        if let Some(started) = self.pressed_at {
            let held = (ui.input(|i| i.time) - started) as f32 / HOLD_SECONDS as f32;
            let t = held.clamp(0.0, 1.0);
            if t > 0.05 {
                let steps = 32;
                let points: Vec<egui::Pos2> = (0..=steps)
                    .map(|i| {
                        let a = -std::f32::consts::FRAC_PI_2
                            + std::f32::consts::TAU * t * i as f32 / steps as f32;
                        egui::pos2(
                            rect.center().x + (r + 3.0) * a.cos(),
                            rect.center().y + (r + 3.0) * a.sin(),
                        )
                    })
                    .collect();
                painter.add(egui::Shape::line(points, Stroke::new(2.0, p.accent)));
            }
        }

        // On a colour, the glyph has to survive whatever the colour is.
        let glyph = if painting {
            let luma = 0.299 * s.brush.paint_color.x
                + 0.587 * s.brush.paint_color.y
                + 0.114 * s.brush.paint_color.z;
            if luma > 0.55 { Color32::from_rgb(20, 20, 20) } else { Color32::WHITE }
        } else {
            p.text
        };
        icons::paint(
            painter,
            icons::centered_rect(rect, r * 1.05),
            Icon::of_brush(s.brush.kind),
            glyph,
        );
    }

    // ---- orbit, pan and zoom -------------------------------------------------

    fn nav_cluster(
        &mut self,
        ctx: &egui::Context,
        viewport: Rect,
        p: &Palette,
        actions: &mut Vec<HudAction>,
    ) {
        let round = self.size * 0.86;
        let gap = 8.0;
        let total = Vec2::new(round, round * 3.0 + gap * 2.0);
        let rect = self.place(viewport, self.nav_at, total);

        egui::Area::new(egui::Id::new("hud_nav"))
            .fixed_pos(rect.min)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                ui.set_min_size(total);
                let arranging = self.arrange_drag(ui, rect, viewport, Cluster::Nav, p);

                let buttons = [
                    (Icon::Twist, "Drag to spin the view"),
                    (Icon::Move, "Drag to pan"),
                    (Icon::Scale, "Drag to zoom"),
                ];
                for (i, (icon, tip)) in buttons.into_iter().enumerate() {
                    let at = Rect::from_min_size(
                        egui::pos2(rect.left(), rect.top() + (round + gap) * i as f32),
                        Vec2::splat(round),
                    );
                    let response = if arranging {
                        ui.interact(at, ui.id().with(("nav_ghost", i)), Sense::hover())
                    } else {
                        ui.interact(at, ui.id().with(("nav", i)), Sense::click_and_drag())
                    };
                    let active = response.dragged();
                    ui.painter().circle(
                        at.center(),
                        at.width() * 0.5,
                        if active { p.accent_dim } else { p.panel.gamma_multiply(0.92) },
                        Stroke::new(
                            1.2,
                            if active || response.hovered() { p.accent } else { p.line },
                        ),
                    );
                    icons::paint(
                        ui.painter(),
                        icons::centered_rect(at, at.width() * 0.5),
                        icon,
                        if active || response.hovered() { p.text } else { p.dim },
                    );
                    if !arranging {
                        let d = response.drag_delta();
                        if response.dragged() && d != Vec2::ZERO {
                            actions.push(match i {
                                0 => HudAction::Orbit(d),
                                1 => HudAction::Pan(d),
                                // Up zooms in, matching the pills and the pad.
                                _ => HudAction::Zoom(-d.y * 0.02),
                            });
                        }
                        if response.hovered() {
                            response.on_hover_text(tip);
                        }
                    }
                }
            });
    }
}

// Layout helpers for the three-part brush cluster.

fn left_rect(rect: Rect, pill: Vec2) -> Rect {
    Rect::from_min_size(egui::pos2(rect.left(), rect.center().y - pill.y * 0.5), pill)
}

fn right_rect(rect: Rect, pill: Vec2) -> Rect {
    Rect::from_min_size(
        egui::pos2(rect.right() - pill.x, rect.center().y - pill.y * 0.5),
        pill,
    )
}

fn centre_rect(rect: Rect, round: f32) -> Rect {
    Rect::from_center_size(rect.center(), Vec2::splat(round))
}

