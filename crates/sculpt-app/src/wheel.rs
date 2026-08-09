//! Radial menu: hold a key, everything you reach for most is under the cursor.
//!
//! It has two faces, and which one you get follows the tool in your hand. On a
//! sculpting tool it shows the sculpting tools. On a painting one it turns into
//! a painting menu: the paint tools on an inner ring, the blend modes on the
//! outer one, and the colour wheel beside it. Picking a sculpting tool takes
//! you back. Showing blend modes to someone holding Clay would be noise, and a
//! colour wheel they cannot use even more so.
//!
//! Nothing responds to hover alone. Sweeping the pointer across the menu on the
//! way somewhere else must not silently change the tool, the size or the
//! colour, so every change here takes a deliberate press or drag.

use crate::icons::{self, Icon};
use crate::theme::Palette;
use egui::{Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use sculpt_core::{BlendMode, BrushKind, Sculptor};

/// Radius of the central pad, as a fraction of the menu's outer radius.
const PAD_FRACTION: f32 = 0.30;
/// Full-scale travel for a pad drag, in points.
const PAD_TRAVEL: f32 = 260.0;

/// Sculpting tools, in the order they sit on the ring.
const SCULPT_TOOLS: [BrushKind; 12] = [
    BrushKind::Draw,
    BrushKind::Clay,
    BrushKind::Flatten,
    BrushKind::Smooth,
    BrushKind::Pinch,
    BrushKind::Crease,
    BrushKind::Inflate,
    BrushKind::Move,
    BrushKind::Drag,
    BrushKind::Twist,
    BrushKind::Scale,
    BrushKind::Mask,
];

/// Painting tools, plus the way back to sculpting.
const PAINT_TOOLS: [BrushKind; 5] = [
    BrushKind::Paint,
    BrushKind::Smudge,
    BrushKind::ColorBlur,
    BrushKind::Fill,
    BrushKind::Mask,
];

/// What the pointer took hold of when the button went down.
enum Grab {
    None,
    /// Dragging the centre pad, with the values it started from.
    Pad { origin: Pos2, radius: f32, strength: f32 },
    /// Dragging inside the colour wheel.
    Colour,
}

/// One slot on a ring.
struct Slot {
    icon: Option<Icon>,
    label: &'static str,
    selected: bool,
}

pub struct Wheel {
    pub open: bool,
    /// Outer radius of the menu, in points.
    pub size: f32,
    /// Show the colour wheel when a painting tool is in hand.
    pub show_colors: bool,
    /// Where the menu was summoned.
    center: Pos2,
    grab: Grab,
    /// Sculpting tool to come back to when leaving the painting menu.
    last_sculpt: BrushKind,
}

impl Default for Wheel {
    fn default() -> Self {
        Self {
            open: false,
            size: 168.0,
            show_colors: true,
            center: Pos2::ZERO,
            grab: Grab::None,
            last_sculpt: BrushKind::Clay,
        }
    }
}

impl Wheel {
    pub fn open_at(&mut self, at: Pos2, s: &Sculptor) {
        self.open = true;
        self.center = at;
        self.grab = Grab::None;
        if !s.brush.kind.paints() {
            self.last_sculpt = s.brush.kind;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.grab = Grab::None;
    }

    /// True while the centre pad is being dragged, which is when the viewport
    /// should be showing what the brush now looks like.
    pub fn is_adjusting(&self) -> bool {
        self.open && matches!(self.grab, Grab::Pad { .. })
    }

    /// Draws the menu and applies what the pointer presses or drags.
    pub fn show(&mut self, ctx: &egui::Context, s: &mut Sculptor, p: &Palette) {
        if !self.open {
            return;
        }
        let painting = s.brush.kind.paints();
        let with_colors = painting && self.show_colors;

        // The whole window, not just the viewport: the menu floats over the
        // docks and dims them with everything else.
        let screen = ctx.viewport_rect();
        // The two discs must not touch: this menu is this radius, the colour
        // wheel is 0.92 of it, and they need air between them.
        let colour_gap = self.size * 2.1;
        let colour_reach = if with_colors { colour_gap + self.size } else { 0.0 };
        let margin = self.size + 20.0;

        // Put the colour wheel on whichever side has room for it.
        let colours_right = !with_colors || screen.right() - self.center.x > colour_reach + 20.0;
        let (left_need, right_need) = if colours_right {
            (margin, margin + colour_reach)
        } else {
            (margin + colour_reach, margin)
        };
        let center = Pos2::new(
            self.center.x.clamp(
                screen.left() + left_need,
                (screen.right() - right_need).max(screen.left() + left_need),
            ),
            self.center.y.clamp(
                screen.top() + margin,
                (screen.bottom() - margin).max(screen.top() + margin),
            ),
        );

        egui::Area::new(egui::Id::new("radial_menu"))
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                let painter = ui.painter();
                painter.rect_filled(screen, 0.0, Color32::from_black_alpha(120));

                let pointer = ui.ctx().pointer_latest_pos().unwrap_or(center);
                let (pressed, down) = ui
                    .ctx()
                    .input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down()));
                if !down {
                    self.grab = Grab::None;
                }

                let colour_center = with_colors.then(|| {
                    let dx = if colours_right { colour_gap } else { -colour_gap };
                    Pos2::new(center.x + dx, center.y)
                });

                // Decide what a fresh press took hold of, once.
                if pressed {
                    let pad = self.size * PAD_FRACTION;
                    let on_colour = colour_center
                        .is_some_and(|c| (pointer - c).length() <= self.size * 0.95);
                    self.grab = if (pointer - center).length() <= pad {
                        Grab::Pad {
                            origin: pointer,
                            radius: s.brush.radius,
                            strength: s.brush.strength,
                        }
                    } else if on_colour {
                        Grab::Colour
                    } else {
                        Grab::None
                    };
                }

                painter.circle_filled(center, self.size, p.panel.gamma_multiply(0.96));
                painter.circle_stroke(center, self.size, Stroke::new(1.0, p.line));

                if painting {
                    self.draw_paint_page(painter, center, pointer, pressed, s, p);
                } else {
                    self.draw_sculpt_page(painter, center, pointer, pressed, s, p);
                }
                self.draw_pad(painter, center, pointer, s, p);
                self.draw_gauges(painter, center, s, p);

                if let Some(c) = colour_center {
                    self.draw_colors(painter, c, pointer, s, p);
                }
            });
    }

    // ---- pages ---------------------------------------------------------------

    fn draw_sculpt_page(
        &mut self,
        painter: &Painter,
        center: Pos2,
        pointer: Pos2,
        pressed: bool,
        s: &mut Sculptor,
        p: &Palette,
    ) {
        let mut slots: Vec<Slot> = SCULPT_TOOLS
            .iter()
            .map(|k| Slot {
                icon: Some(Icon::of_brush(*k)),
                label: k.label(),
                selected: s.brush.kind == *k,
            })
            .collect();
        // One door through to the painting menu.
        slots.push(Slot { icon: Some(Icon::Paint), label: "Paint", selected: false });

        let hit = self.ring(painter, center, pointer, pressed, 0.50, 1.0, &slots, p);
        if let Some(i) = hit {
            if i < SCULPT_TOOLS.len() {
                self.last_sculpt = SCULPT_TOOLS[i];
                s.set_brush_kind(SCULPT_TOOLS[i]);
            } else {
                s.set_brush_kind(BrushKind::Paint);
            }
        }
    }

    fn draw_paint_page(
        &mut self,
        painter: &Painter,
        center: Pos2,
        pointer: Pos2,
        pressed: bool,
        s: &mut Sculptor,
        p: &Palette,
    ) {
        // Outer ring: how the colour combines with what is already there.
        let blends: Vec<Slot> = BlendMode::ALL
            .iter()
            .map(|b| Slot {
                icon: None,
                label: b.label(),
                selected: s.brush.blend == *b,
            })
            .collect();
        if let Some(i) = self.ring(painter, center, pointer, pressed, 0.74, 1.0, &blends, p) {
            s.brush.blend = BlendMode::ALL[i];
        }

        // Inner ring: the painting tools, and the way back to sculpting.
        let mut tools: Vec<Slot> = PAINT_TOOLS
            .iter()
            .map(|k| Slot {
                icon: Some(Icon::of_brush(*k)),
                label: k.label(),
                selected: s.brush.kind == *k,
            })
            .collect();
        tools.push(Slot {
            icon: Some(Icon::of_brush(self.last_sculpt)),
            label: "Sculpt",
            selected: false,
        });

        if let Some(i) = self.ring(painter, center, pointer, pressed, 0.40, 0.70, &tools, p) {
            if i < PAINT_TOOLS.len() {
                s.set_brush_kind(PAINT_TOOLS[i]);
            } else {
                s.set_brush_kind(self.last_sculpt);
            }
        }
    }

    /// Draws one ring of slots and reports the index a press landed on.
    ///
    /// Radii are fractions of the menu's outer radius.
    fn ring(
        &self,
        painter: &Painter,
        center: Pos2,
        pointer: Pos2,
        pressed: bool,
        r_in: f32,
        r_out: f32,
        slots: &[Slot],
        p: &Palette,
    ) -> Option<usize> {
        let n = slots.len();
        if n == 0 {
            return None;
        }
        let (r_in, r_out) = (self.size * r_in, self.size * r_out);
        let step = std::f32::consts::TAU / n as f32;
        // Start at the top and go clockwise, which is how a clock face reads.
        let angle_of = |i: usize| -std::f32::consts::FRAC_PI_2 + step * i as f32;

        let offset = pointer - center;
        let distance = offset.length();
        let hovered = (distance >= r_in && distance <= r_out).then(|| {
            let a = offset.y.atan2(offset.x) + std::f32::consts::FRAC_PI_2;
            ((a.rem_euclid(std::f32::consts::TAU) / step).round() as usize) % n
        });

        let mid = (r_in + r_out) * 0.5;
        let slot_size = (r_out - r_in) * 0.88;
        for (i, slot) in slots.iter().enumerate() {
            let a = angle_of(i);
            let at = Pos2::new(center.x + mid * a.cos(), center.y + mid * a.sin());
            let over = hovered == Some(i);
            if slot.selected || over {
                painter.circle_filled(
                    at,
                    slot_size * 0.5,
                    if slot.selected { p.accent_dim } else { p.raised },
                );
            }
            if slot.selected {
                painter.circle_stroke(at, slot_size * 0.5, Stroke::new(1.5, p.accent));
            }
            let colour = if slot.selected {
                p.accent
            } else if over {
                p.text
            } else {
                p.dim
            };
            match slot.icon {
                Some(icon) => {
                    let cell = Rect::from_center_size(at, Vec2::splat(slot_size));
                    icons::paint(
                        painter,
                        icons::centered_rect(cell, slot_size * 0.62),
                        icon,
                        colour,
                    );
                }
                None => {
                    painter.text(
                        at,
                        Align2::CENTER_CENTER,
                        slot.label,
                        FontId::proportional((slot_size * 0.3).clamp(9.0, 13.0)),
                        colour,
                    );
                }
            }
        }

        // Name whatever is under the pointer, just outside the menu.
        if let Some(i) = hovered {
            painter.text(
                Pos2::new(center.x, center.y - self.size - 14.0),
                Align2::CENTER_CENTER,
                slots[i].label,
                FontId::proportional(13.0),
                p.accent,
            );
        }
        pressed.then_some(hovered).flatten()
    }

    // ---- pad and gauges ------------------------------------------------------

    /// The centre pad: drag up and down for radius, left and right for strength.
    fn draw_pad(
        &mut self,
        painter: &Painter,
        center: Pos2,
        pointer: Pos2,
        s: &mut Sculptor,
        p: &Palette,
    ) {
        let pad = self.size * PAD_FRACTION;
        let dragging = matches!(self.grab, Grab::Pad { .. });
        let hovering = (pointer - center).length() <= pad;

        painter.circle_filled(center, pad, p.bg.gamma_multiply(0.94));
        painter.circle_stroke(
            center,
            pad,
            Stroke::new(
                if dragging { 2.0 } else { 1.0 },
                if dragging {
                    p.accent
                } else if hovering {
                    p.text
                } else {
                    p.line
                },
            ),
        );

        if let Grab::Pad { origin, radius, strength } = self.grab {
            // Measured from where the drag began, not from the centre, so the
            // gesture can always be walked back to where it started.
            let travel = pointer - origin;
            let up = (-travel.y / PAD_TRAVEL).clamp(-1.0, 1.0);
            let right = (travel.x / PAD_TRAVEL).clamp(-1.0, 1.0);
            // Radius moves geometrically: a given distance should feel the same
            // whether the brush is tiny or huge.
            s.brush.radius = (radius * 8f32.powf(up)).clamp(0.003, 1.5);
            s.brush.strength = (strength + right).clamp(0.0, 1.0);
            painter.line_segment(
                [origin, pointer],
                Stroke::new(1.0, p.accent.gamma_multiply(0.5)),
            );
        }

        let font = FontId::proportional(pad * 0.26);
        painter.text(
            Pos2::new(center.x, center.y - pad * 0.28),
            Align2::CENTER_CENTER,
            format!("{:.3}", s.brush.radius),
            font.clone(),
            if dragging { p.accent } else { p.text },
        );
        painter.text(
            Pos2::new(center.x, center.y + pad * 0.04),
            Align2::CENTER_CENTER,
            format!("{:.2}", s.brush.strength),
            font,
            if dragging { p.accent } else { p.dim },
        );
        painter.text(
            Pos2::new(center.x, center.y + pad * 0.46),
            Align2::CENTER_CENTER,
            if dragging { "size / force" } else { "drag me" },
            FontId::proportional(pad * 0.18),
            p.faint,
        );
    }

    /// Arcs outside the menu reading the two values the pad drives.
    fn draw_gauges(&self, painter: &Painter, center: Pos2, s: &Sculptor, p: &Palette) {
        let arc = |from: f32, to: f32, radius: f32, width: f32, colour: Color32| {
            let steps = 48;
            let points: Vec<Pos2> = (0..=steps)
                .map(|i| {
                    let t = from + (to - from) * i as f32 / steps as f32;
                    Pos2::new(center.x + radius * t.cos(), center.y + radius * t.sin())
                })
                .collect();
            painter.add(Shape::line(points, Stroke::new(width, colour)));
        };
        use std::f32::consts::PI;
        let r = self.size + 9.0;
        let radius_t = (s.brush.radius / 1.5).clamp(0.0, 1.0).powf(0.4);
        arc(PI * 0.62, PI * 1.38, r, 3.0, p.line);
        arc(PI * 1.38, PI * 1.38 - PI * 0.76 * radius_t, r, 3.0, p.accent);

        let strength_t = s.brush.strength.clamp(0.0, 1.0);
        arc(-PI * 0.38, PI * 0.38, r, 3.0, p.line);
        arc(-PI * 0.38, -PI * 0.38 + PI * 0.76 * strength_t, r, 3.0, p.accent);
    }

    // ---- colour wheel --------------------------------------------------------

    fn draw_colors(
        &self,
        painter: &Painter,
        center: Pos2,
        pointer: Pos2,
        s: &mut Sculptor,
        p: &Palette,
    ) {
        let outer = self.size * 0.92;
        let inner = outer * 0.78;
        let current = s.brush.paint_color;
        let (mut hue, mut sat, mut val) = rgb_to_hsv(current.x, current.y, current.z);

        // Hue ring, drawn as a fan of coloured wedges.
        let steps = 96;
        for i in 0..steps {
            let a0 = i as f32 / steps as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / steps as f32 * std::f32::consts::TAU;
            let (r, g, b) = hsv_to_rgb(i as f32 / steps as f32, 1.0, 1.0);
            let colour =
                Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);
            let quad = vec![
                Pos2::new(center.x + inner * a0.cos(), center.y + inner * a0.sin()),
                Pos2::new(center.x + outer * a0.cos(), center.y + outer * a0.sin()),
                Pos2::new(center.x + outer * a1.cos(), center.y + outer * a1.sin()),
                Pos2::new(center.x + inner * a1.cos(), center.y + inner * a1.sin()),
            ];
            painter.add(Shape::convex_polygon(quad, colour, Stroke::NONE));
        }

        // The saturation and value square, inscribed in the ring.
        //
        // One quad with four corner colours is exactly right here: value times
        // the white-to-hue ramp is bilinear in the two axes, so the hardware
        // interpolator draws the whole square for free.
        let half = inner * 0.68;
        let square = Rect::from_center_size(center, Vec2::splat(half * 2.0));
        let (hr, hg, hb) = hsv_to_rgb(hue, 1.0, 1.0);
        let hue_colour =
            Color32::from_rgb((hr * 255.0) as u8, (hg * 255.0) as u8, (hb * 255.0) as u8);
        let mut mesh = Mesh::default();
        mesh.colored_vertex(square.left_bottom(), Color32::BLACK);
        mesh.colored_vertex(square.right_bottom(), Color32::BLACK);
        mesh.colored_vertex(square.left_top(), Color32::WHITE);
        mesh.colored_vertex(square.right_top(), hue_colour);
        mesh.add_triangle(0, 1, 3);
        mesh.add_triangle(0, 3, 2);
        painter.add(Shape::mesh(mesh));
        painter.rect_stroke(
            square,
            egui::CornerRadius::same(2),
            Stroke::new(1.0, p.line),
            egui::StrokeKind::Outside,
        );

        // Colour follows the pointer only while it is being dragged here, so
        // crossing the wheel on the way past leaves the colour alone.
        let offset = pointer - center;
        if matches!(self.grab, Grab::Colour) {
            if offset.length() > inner * 0.9 {
                hue = (offset.y.atan2(offset.x) / std::f32::consts::TAU).rem_euclid(1.0);
            } else {
                sat = ((pointer.x - square.left()) / square.width()).clamp(0.0, 1.0);
                val = ((square.bottom() - pointer.y) / square.height()).clamp(0.0, 1.0);
            }
            let (r, g, b) = hsv_to_rgb(hue, sat, val);
            s.brush.paint_color = glam::Vec3::new(r, g, b);
        }

        // Markers for where the current colour sits.
        let hue_angle = hue * std::f32::consts::TAU;
        let ring_mid = (inner + outer) * 0.5;
        painter.circle_stroke(
            Pos2::new(
                center.x + ring_mid * hue_angle.cos(),
                center.y + ring_mid * hue_angle.sin(),
            ),
            (outer - inner) * 0.42,
            Stroke::new(2.0, p.text),
        );
        painter.circle_stroke(
            Pos2::new(
                square.left() + sat * square.width(),
                square.bottom() - val * square.height(),
            ),
            5.0,
            Stroke::new(1.6, if val > 0.55 { Color32::BLACK } else { Color32::WHITE }),
        );

        let (r, g, b) = hsv_to_rgb(hue, sat, val);
        painter.text(
            Pos2::new(center.x, center.y + outer + 14.0),
            Align2::CENTER_CENTER,
            format!(
                "#{:02X}{:02X}{:02X}",
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8
            ),
            FontId::monospace(12.0),
            p.text,
        );
    }
}

// ---------------------------------------------------------------------------
// colour conversion
// ---------------------------------------------------------------------------

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r + m, g + m, b + m)
}

pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if max == r {
        (((g - b) / d) % 6.0) / 6.0
    } else if max == g {
        (((b - r) / d) + 2.0) / 6.0
    } else {
        (((r - g) / d) + 4.0) / 6.0
    };
    let s = if max < 1e-6 { 0.0 } else { d / max };
    (h.rem_euclid(1.0), s, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_conversion_round_trips() {
        for (r, g, b) in [
            (1.0, 0.0, 0.0),
            (0.2, 0.7, 0.4),
            (0.05, 0.05, 0.05),
            (1.0, 1.0, 1.0),
        ] {
            let (h, s, v) = rgb_to_hsv(r, g, b);
            let (r2, g2, b2) = hsv_to_rgb(h, s, v);
            assert!((r - r2).abs() < 1e-3, "{r} became {r2}");
            assert!((g - g2).abs() < 1e-3, "{g} became {g2}");
            assert!((b - b2).abs() < 1e-3, "{b} became {b2}");
        }
    }

    /// Every sculpting tool and every painting tool must be reachable, and the
    /// two lists must not overlap, or a tool would be stranded.
    #[test]
    fn every_tool_has_a_home_on_the_wheel() {
        for kind in BrushKind::ALL {
            let on_sculpt = SCULPT_TOOLS.contains(&kind);
            let on_paint = PAINT_TOOLS.contains(&kind);
            assert!(on_sculpt || on_paint, "{kind:?} is on neither page");
            if kind.paints() {
                assert!(on_paint, "{kind:?} paints but is not on the painting page");
            }
        }
    }
}
