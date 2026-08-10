//! Floating controls: the things a hand on a tablet needs within reach.
//!
//! Five of them, each its own button with its own place and its own size.
//! Beside the tool rail a column of three: size, the menu, force. On the other
//! side, pan and zoom, which a tablet has no wheel or middle button for. There
//! is no orbit button because the orientation ball above already spins the
//! model, and two ways to do one thing in the same corner is one too many.
//!
//! Positions are fractions of the viewport, so they hold their place when the
//! window resizes, and Arrange lets each one be dragged on its own.

use crate::icons::{self, Icon};
use crate::theme::Palette;
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Vec2};
use sculpt_core::Sculptor;

/// How long the menu button is held before the radial menu appears.
const HOLD_SECONDS: f64 = 0.28;
/// Points of vertical drag for a pill to travel its whole range.
const PILL_TRAVEL: f32 = 220.0;

#[derive(Clone, Copy, Debug)]
pub enum HudAction {
    /// Open the radial menu. Where it goes is the caller's business.
    OpenWheel,
    Pan(Vec2),
    Zoom(f32),
}

/// The five floating controls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PodKind {
    Size,
    Menu,
    Force,
    Pan,
    Zoom,
}

impl PodKind {
    pub const ALL: [PodKind; 5] =
        [PodKind::Size, PodKind::Menu, PodKind::Force, PodKind::Pan, PodKind::Zoom];

    pub fn label(self) -> &'static str {
        match self {
            PodKind::Size => "Size",
            PodKind::Menu => "Menu",
            PodKind::Force => "Force",
            PodKind::Pan => "Pan",
            PodKind::Zoom => "Zoom",
        }
    }

    /// Tall pills for the two values, round buttons for the rest.
    fn is_pill(self) -> bool {
        matches!(self, PodKind::Size | PodKind::Force)
    }

    fn default_at(self) -> Vec2 {
        match self {
            // A column beside the rail: size on top, the menu in the middle,
            // force underneath.
            PodKind::Size => Vec2::new(0.055, 0.34),
            PodKind::Menu => Vec2::new(0.055, 0.50),
            PodKind::Force => Vec2::new(0.055, 0.66),
            // Well apart, so a thumb never hits the wrong one.
            PodKind::Pan => Vec2::new(0.95, 0.36),
            PodKind::Zoom => Vec2::new(0.95, 0.50),
        }
    }

    fn default_size(self) -> f32 {
        match self {
            PodKind::Pan | PodKind::Zoom => 64.0,
            _ => 54.0,
        }
    }
}

/// One floating control.
#[derive(Clone, Copy)]
pub struct Pod {
    pub kind: PodKind,
    pub at: Vec2,
    pub size: f32,
    pub visible: bool,
}

impl Pod {
    fn new(kind: PodKind) -> Self {
        Self { kind, at: kind.default_at(), size: kind.default_size(), visible: true }
    }

    /// Footprint in points.
    fn extent(&self) -> Vec2 {
        if self.kind.is_pill() {
            Vec2::new(self.size * 0.78, self.size * 2.6)
        } else {
            Vec2::splat(self.size)
        }
    }
}

pub struct Hud {
    pub pods: Vec<Pod>,
    /// Drag the buttons to move them instead of using them.
    pub arrange: bool,
    /// When the menu button went down, for the hold gesture.
    pressed_at: Option<f64>,
    wheel_open: bool,
    /// True while size or force is being dragged.
    adjusting: bool,
    /// Colour under the cursor while the eyedropper is armed, so the button
    /// shows what a click would take before it takes it.
    picking: Option<glam::Vec3>,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            pods: PodKind::ALL.iter().map(|k| Pod::new(*k)).collect(),
            arrange: false,
            pressed_at: None,
            wheel_open: false,
            adjusting: false,
            picking: None,
        }
    }
}

impl Hud {
    pub fn reset(&mut self) {
        self.pods = PodKind::ALL.iter().map(|k| Pod::new(*k)).collect();
    }

    /// True while size or force is being dragged.
    pub fn is_adjusting(&self) -> bool {
        self.adjusting
    }

    /// Draws every button and reports what they were asked to do.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        viewport: Rect,
        s: &mut Sculptor,
        p: &Palette,
        wheel_open: bool,
        picking: Option<glam::Vec3>,
    ) -> Vec<HudAction> {
        let mut actions = Vec::new();
        self.adjusting = false;
        self.wheel_open = wheel_open;
        self.picking = picking;

        for index in 0..self.pods.len() {
            let pod = self.pods[index];
            if !pod.visible {
                continue;
            }
            let rect = place(viewport, pod.at, pod.extent());
            egui::Area::new(egui::Id::new(("hud_pod", index)))
                .fixed_pos(rect.min)
                .order(egui::Order::Middle)
                .show(ctx, |ui| {
                    ui.set_min_size(rect.size());
                    if self.arrange {
                        self.arrange_pod(ui, index, rect, viewport, s, p);
                    } else {
                        self.live_pod(ui, index, rect, s, p, &mut actions);
                    }
                });
        }
        actions
    }

    /// Arrange mode: the button is scenery, and dragging it moves it.
    fn arrange_pod(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        rect: Rect,
        viewport: Rect,
        s: &Sculptor,
        p: &Palette,
    ) {
        let response = ui.interact(rect, ui.id().with("arrange"), Sense::click_and_drag());
        self.paint_pod(ui, index, rect, s, p, false);
        ui.painter().rect(
            rect.expand(4.0),
            CornerRadius::same(14),
            Color32::TRANSPARENT,
            Stroke::new(
                1.5,
                p.accent
                    .gamma_multiply(if response.hovered() || response.dragged() { 1.0 } else { 0.45 }),
            ),
            egui::StrokeKind::Inside,
        );
        if response.dragged() {
            let d = response.drag_delta();
            let pod = &mut self.pods[index];
            pod.at.x = (pod.at.x + d.x / viewport.width().max(1.0)).clamp(0.0, 1.0);
            pod.at.y = (pod.at.y + d.y / viewport.height().max(1.0)).clamp(0.0, 1.0);
        }
        ui.painter().text(
            rect.center_bottom() + Vec2::new(0.0, 12.0),
            Align2::CENTER_CENTER,
            self.pods[index].kind.label(),
            FontId::proportional(10.0),
            p.accent,
        );
    }

    /// Normal mode: the button does its job.
    fn live_pod(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        rect: Rect,
        s: &mut Sculptor,
        p: &Palette,
        actions: &mut Vec<HudAction>,
    ) {
        let kind = self.pods[index].kind;
        match kind {
            PodKind::Size | PodKind::Force => {
                let response = ui.interact(rect, ui.id().with("pill"), Sense::click_and_drag());
                // Acts on a resting finger too, so a pause in the drag is not
                // mistaken for letting go.
                if response.dragged() || response.is_pointer_button_down_on() {
                    let t = -response.drag_delta().y / PILL_TRAVEL;
                    if kind == PodKind::Size {
                        // Size travels geometrically: the same distance should
                        // feel the same whether the brush is tiny or huge.
                        s.brush.radius = (s.brush.radius * 8f32.powf(t)).clamp(0.003, 1.5);
                    } else {
                        s.brush.strength = (s.brush.strength + t).clamp(0.0, 1.0);
                    }
                    self.adjusting = true;
                }
                self.paint_pod(ui, index, rect, s, p, response.hovered());
            }
            PodKind::Menu => {
                let response = ui.interact(rect, ui.id().with("menu"), Sense::click_and_drag());
                let now = ui.input(|i| i.time);
                // This button only opens the menu; closing belongs to the menu.
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
                self.paint_pod(ui, index, rect, s, p, response.hovered());
            }
            PodKind::Pan | PodKind::Zoom => {
                let response = ui.interact(rect, ui.id().with("nav"), Sense::click_and_drag());
                let d = response.drag_delta();
                if response.dragged() && d != Vec2::ZERO {
                    actions.push(match kind {
                        PodKind::Pan => HudAction::Pan(d),
                        // Up zooms in, as it does everywhere else here.
                        _ => HudAction::Zoom(-d.y * 0.02),
                    });
                }
                let hot = response.hovered() || response.dragged();
                self.paint_pod(ui, index, rect, s, p, hot);
                if response.hovered() {
                    let tip = if kind == PodKind::Pan { "Drag to pan" } else { "Drag to zoom" };
                    response.on_hover_text(tip);
                }
            }
        }
    }

    fn paint_pod(
        &self,
        ui: &egui::Ui,
        index: usize,
        rect: Rect,
        s: &Sculptor,
        p: &Palette,
        hot: bool,
    ) {
        let pod = self.pods[index];
        match pod.kind {
            PodKind::Size => {
                self.paint_pill(ui, rect, "Size", s.brush.radius, 0.003, 1.5, true, p, hot)
            }
            PodKind::Force => {
                self.paint_pill(ui, rect, "Force", s.brush.strength, 0.0, 1.0, false, p, hot)
            }
            PodKind::Menu => self.paint_menu_button(ui, rect, s, p, hot),
            PodKind::Pan | PodKind::Zoom => {
                let icon = if pod.kind == PodKind::Pan { Icon::Move } else { Icon::Scale };
                let painter = ui.painter();
                painter.circle(
                    rect.center(),
                    rect.width() * 0.5,
                    if hot { p.accent_dim } else { p.panel.gamma_multiply(0.92) },
                    Stroke::new(1.2, if hot { p.accent } else { p.line }),
                );
                icons::paint(
                    painter,
                    icons::centered_rect(rect, rect.width() * 0.5),
                    icon,
                    if hot { p.text } else { p.dim },
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_pill(
        &self,
        ui: &egui::Ui,
        rect: Rect,
        label: &str,
        value: f32,
        lo: f32,
        hi: f32,
        logarithmic: bool,
        p: &Palette,
        hot: bool,
    ) {
        let painter = ui.painter();
        let radius = CornerRadius::same((rect.width() * 0.5) as u8);
        painter.rect_filled(rect, radius, p.panel.gamma_multiply(0.92));
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(if hot { 1.6 } else { 1.0 }, if hot { p.accent } else { p.line }),
            egui::StrokeKind::Inside,
        );

        let t = if logarithmic {
            ((value.max(1e-6) / lo.max(1e-6)).ln() / (hi / lo.max(1e-6)).ln()).clamp(0.0, 1.0)
        } else {
            ((value - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0)
        };
        let filled =
            Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - rect.height() * t), rect.max);
        if filled.height() > 2.0 {
            painter.rect_filled(filled, radius, p.accent_dim);
        }

        let font = FontId::proportional((rect.width() * 0.24).clamp(8.0, 13.0));
        painter.text(
            egui::pos2(rect.center().x, rect.top() + 12.0),
            Align2::CENTER_CENTER,
            label,
            font.clone(),
            p.dim,
        );
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() - 12.0),
            Align2::CENTER_CENTER,
            if logarithmic { format!("{value:.3}") } else { format!("{value:.2}") },
            font,
            p.text,
        );
    }

    /// The round button: the tool in hand, or the colour when that tool paints.
    ///
    /// While the eyedropper is armed it shows the colour under the cursor
    /// instead, with its hex underneath. Picking a colour you cannot see until
    /// after you have taken it is guesswork.
    fn paint_menu_button(&self, ui: &egui::Ui, rect: Rect, s: &Sculptor, p: &Palette, hot: bool) {
        let painter = ui.painter();
        let r = rect.width() * 0.5;
        let painting = s.brush.kind.paints();
        let shown = self.picking.or(painting.then_some(s.brush.paint_color));
        let fill = match shown {
            Some(c) => Color32::from_rgb(
                (c.x.clamp(0.0, 1.0) * 255.0) as u8,
                (c.y.clamp(0.0, 1.0) * 255.0) as u8,
                (c.z.clamp(0.0, 1.0) * 255.0) as u8,
            ),
            None => p.raised,
        };
        painter.circle(
            rect.center(),
            r,
            fill,
            Stroke::new(if hot { 2.0 } else { 1.2 }, if hot { p.accent } else { p.line }),
        );

        // A held button fills its ring as the menu arms.
        if let Some(started) = self.pressed_at {
            let t = ((ui.input(|i| i.time) - started) as f32 / HOLD_SECONDS as f32).clamp(0.0, 1.0);
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
        let glyph = match shown {
            Some(c) => {
                let luma = 0.299 * c.x + 0.587 * c.y + 0.114 * c.z;
                if luma > 0.55 { Color32::from_rgb(20, 20, 20) } else { Color32::WHITE }
            }
            None => p.text,
        };
        icons::paint(
            painter,
            icons::centered_rect(rect, r * 1.05),
            if self.picking.is_some() { Icon::Palette } else { Icon::of_brush(s.brush.kind) },
            glyph,
        );

        // The hex, on a plate under the button, so the number and the patch are
        // read in one look.
        if let Some(c) = shown {
            let text = format!(
                "#{:02X}{:02X}{:02X}",
                (c.x.clamp(0.0, 1.0) * 255.0) as u8,
                (c.y.clamp(0.0, 1.0) * 255.0) as u8,
                (c.z.clamp(0.0, 1.0) * 255.0) as u8
            );
            let font = FontId::monospace((rect.width() * 0.2).clamp(8.0, 11.0));
            let galley = painter.layout_no_wrap(text, font, p.text);
            let plate = Rect::from_center_size(
                egui::pos2(rect.center().x, rect.bottom() + 10.0),
                galley.size() + Vec2::new(10.0, 4.0),
            );
            painter.rect(
                plate,
                CornerRadius::same((plate.height() * 0.5) as u8),
                p.panel.gamma_multiply(0.92),
                Stroke::new(1.0, p.line),
                egui::StrokeKind::Inside,
            );
            painter.galley(
                egui::pos2(plate.center().x - galley.size().x * 0.5, plate.center().y - galley.size().y * 0.5),
                galley,
                p.text,
            );
        }
    }
}

/// Centres a footprint on a fraction of the viewport, kept fully inside it.
fn place(viewport: Rect, at: Vec2, size: Vec2) -> Rect {
    let centre = egui::pos2(
        viewport.left() + viewport.width() * at.x,
        viewport.top() + viewport.height() * at.y,
    );
    let half = size * 0.5;
    let clamped = egui::pos2(
        centre.x.clamp(
            viewport.left() + half.x,
            (viewport.right() - half.x).max(viewport.left() + half.x),
        ),
        centre.y.clamp(
            viewport.top() + half.y,
            (viewport.bottom() - half.y).max(viewport.top() + half.y),
        ),
    );
    Rect::from_center_size(clamped, size)
}
