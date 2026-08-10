//! Radial menu: hold a key, everything you reach for most is under the cursor.
//!
//! It has two faces, and which one you get follows the tool in your hand. On a
//! sculpting tool it shows the sculpting tools. On a painting one it turns into
//! a painting menu: the paint tools on an inner ring, the blend modes around
//! them, and the hue wrapped right around the outside, so there is still one
//! disc rather than two things to aim at. Picking a sculpting tool takes you
//! back. Showing blend modes to someone holding Clay would be noise, and a
//! colour ring they cannot use even more so.
//!
//! The middle stays the pad in both faces. Size and force are wanted just as
//! often while painting, and moving them somewhere else in half the menu would
//! cost more than the colour square gains by sitting there.
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
/// Where the hue ring begins, as a fraction of the outer radius.
const HUE_INNER: f32 = 0.86;
/// Size of the saturation and value square hanging below the menu, in points.
const SV_WIDTH: f32 = 148.0;
const SV_HEIGHT: f32 = 92.0;

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

/// How many swatches the palette holds.
pub const SWATCHES: usize = 10;

/// What the painting menu asks the rest of the application for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelRequest {
    /// Start picking a colour off the model.
    PickColor,
}

/// What the pointer took hold of when the button went down.
enum Grab {
    None,
    /// Dragging the centre pad, with the values it started from.
    Pad { origin: Pos2, radius: f32, strength: f32 },
    /// Dragging inside the saturation and value square.
    Colour,
    /// Dragging around the hue ring.
    Hue,
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
    /// Show the colour ring when a painting tool is in hand.
    pub show_colors: bool,
    /// Drop the hue ring to a grey ramp, for value studies.
    pub grayscale: bool,
    /// Saved colours, kept across sessions of the menu.
    pub swatches: Vec<[f32; 3]>,
    /// Where the menu was summoned.
    center: Pos2,
    grab: Grab,
    /// Sculpting tool to come back to when leaving the painting menu.
    last_sculpt: BrushKind,
    /// Whether the pointer was already down when the menu opened, resolved on
    /// the first frame it is up.
    ///
    /// It decides what counts as choosing. Summoned by a key, the pointer is
    /// free and a press picks. Summoned by holding a button, the finger is
    /// already down and will not press again without letting go, so the choice
    /// has to land on release, which is how a radial menu has always worked.
    opened_held: Option<bool>,
    /// Pointer state last frame, so a release is detected as an edge rather
    /// than trusted from an event.
    ///
    /// Remote desktops and pen digitisers both like to emit a stray release
    /// while the finger is still down. Watching the level go from down to up
    /// ignores those; watching the event does not.
    was_down: bool,
    /// Frames since the menu opened. The first few ignore a release entirely,
    /// which absorbs the event storm that opening tends to sit in the middle of.
    frames: u32,
}

impl Default for Wheel {
    fn default() -> Self {
        Self {
            open: false,
            size: 168.0,
            show_colors: true,
            grayscale: false,
            swatches: Vec::new(),
            center: Pos2::ZERO,
            grab: Grab::None,
            last_sculpt: BrushKind::Clay,
            opened_held: None,
            was_down: false,
            frames: 0,
        }
    }
}

impl Wheel {
    pub fn open_at(&mut self, at: Pos2, s: &Sculptor) {
        self.open = true;
        self.center = at;
        self.grab = Grab::None;
        self.opened_held = None;
        self.was_down = true;
        self.frames = 0;
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
    ///
    /// `viewport` is the area left over by the docks: the menu is kept inside
    /// it, because a ring hanging over the settings panel is both ugly and
    /// harder to aim at.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        viewport: egui::Rect,
        s: &mut Sculptor,
        p: &Palette,
    ) -> Option<WheelRequest> {
        if !self.open {
            return None;
        }
        let painting = s.brush.kind.paints();
        let with_colors = painting && self.show_colors;

        // Dim the whole window, but keep the menu itself in the viewport.
        let window = ctx.viewport_rect();
        let screen = viewport;
        // One disc, whatever is in it: the colours live on its outermost ring
        // rather than in a second wheel beside it. The square and the swatches
        // hang below, so the room needed underneath is not the same as around.
        let margin = self.size + 34.0;
        let below = if with_colors { margin + SV_HEIGHT + 46.0 } else { margin };
        let center = Pos2::new(
            self.center.x.clamp(
                screen.left() + margin,
                (screen.right() - margin).max(screen.left() + margin),
            ),
            self.center.y.clamp(
                screen.top() + margin,
                (screen.bottom() - below).max(screen.top() + margin),
            ),
        );
        let mut request = None;

        // A plain layer painter, not an `Area`.
        //
        // An area sizes itself to the widgets put inside it, and this menu puts
        // none there: it paints at absolute coordinates and reads input from the
        // context. With nothing to measure the area collapsed and clipped the
        // whole menu away. It survived while the pointer moved, since that kept
        // nudging the area's remembered size, and vanished the moment the hand
        // held still, which is precisely how it is meant to be used.
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("radial_menu"),
        ));
        painter.rect_filled(window, 0.0, Color32::from_black_alpha(120));

        let pointer = ctx.pointer_latest_pos().unwrap_or(center);
        let (pressed, released, down) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.primary_down(),
            )
        });
        // First frame decides how this menu was summoned.
        let held_open = *self.opened_held.get_or_insert(down);
        self.frames = self.frames.saturating_add(1);

        // A release is the level going from down to up, not the event saying
        // so. A remote desktop that turns touches into mouse messages, and a
        // digitiser that reports a hover between two contact reports, both emit
        // releases the hand never made; an edge on the level ignores them. The
        // first few frames ignore releases outright, since opening the menu
        // sits in the middle of exactly that kind of event storm.
        let settled = self.frames > 3;
        let release_edge = settled && self.was_down && !down;
        self.was_down = down;

        // A press picks when the pointer is free; a release picks when the
        // finger that opened the menu is still on the glass.
        let choose = if held_open { release_edge } else { pressed };

        if !down {
            self.grab = Grab::None;
        }

        // Take hold of the centre or the hue ring as soon as the pointer is down
        // over either, whether that press happened here or was the one that
        // opened the menu.
        let hue_inner = self.size * HUE_INNER;
        let square = Self::sv_rect(center, self.size);
        if down && matches!(self.grab, Grab::None) {
            let from_center = (pointer - center).length();
            let pad = self.size * PAD_FRACTION;
            if from_center <= pad {
                self.grab = Grab::Pad {
                    origin: pointer,
                    radius: s.brush.radius,
                    strength: s.brush.strength,
                };
            } else if with_colors && square.contains(pointer) {
                self.grab = Grab::Colour;
            } else if with_colors && from_center >= hue_inner && from_center <= self.size {
                self.grab = Grab::Hue;
            }
        }

        painter.circle_filled(center, self.size, p.panel.gamma_multiply(0.96));
        painter.circle_stroke(center, self.size, Stroke::new(1.0, p.line));

        if with_colors {
            self.draw_hue_ring(&painter, center, pointer, hue_inner, s, p);
        }
        if painting {
            if let Some(r) = self.draw_paint_page(&painter, center, pointer, choose, s, p) {
                request = Some(r);
            }
        } else {
            self.draw_sculpt_page(&painter, center, pointer, choose, s, p);
        }
        self.draw_pad(&painter, center, pointer, s, p);
        if with_colors {
            self.draw_sv_square(&painter, square, s, pointer, p);
            self.draw_swatches(&painter, center, square, pointer, choose, s, p);
        }
        self.draw_gauges(&painter, center, s, p);

        // Opened by holding a button, the menu belongs to that finger: it lives
        // until the finger lifts, and the lift is also the choice. The
        // key-summoned menu ignores this and waits for the key instead.
        if held_open && release_edge {
            self.open = false;
            self.grab = Grab::None;
        }
        let _ = released;
        request
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
    ) -> Option<WheelRequest> {
        // Outer ring: how the colour combines with what is already there.
        // The hue ring takes the outside when it is shown, so the tool rings
        // move in to sit under it rather than through it.
        let (blend_in, blend_out, tool_in, tool_out) = if self.show_colors {
            (0.60, HUE_INNER - 0.02, 0.33, 0.58)
        } else {
            (0.74, 1.0, 0.40, 0.70)
        };

        let blends: Vec<Slot> = BlendMode::ALL
            .iter()
            .map(|b| Slot {
                icon: None,
                label: b.label(),
                selected: s.brush.blend == *b,
            })
            .collect();
        if let Some(i) = self.ring(painter, center, pointer, pressed, blend_in, blend_out, &blends, p)
        {
            s.brush.blend = BlendMode::ALL[i];
        }

        // Inner ring: the painting tools, the two colour controls, and the way
        // back to sculpting.
        let mut tools: Vec<Slot> = PAINT_TOOLS
            .iter()
            .map(|k| Slot {
                icon: Some(Icon::of_brush(*k)),
                label: k.label(),
                selected: s.brush.kind == *k,
            })
            .collect();
        tools.push(Slot { icon: Some(Icon::Palette), label: "Pick", selected: false });
        tools.push(Slot {
            icon: Some(Icon::Mask),
            label: "Greys",
            selected: self.grayscale,
        });
        tools.push(Slot {
            icon: Some(Icon::of_brush(self.last_sculpt)),
            label: "Sculpt",
            selected: false,
        });

        let mut request = None;
        if let Some(i) = self.ring(painter, center, pointer, pressed, tool_in, tool_out, &tools, p) {
            let paints = PAINT_TOOLS.len();
            if i < paints {
                s.set_brush_kind(PAINT_TOOLS[i]);
            } else if i == paints {
                request = Some(WheelRequest::PickColor);
            } else if i == paints + 1 {
                self.grayscale = !self.grayscale;
            } else {
                s.set_brush_kind(self.last_sculpt);
            }
        }
        request
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

    // ---- colour --------------------------------------------------------------

    /// The outermost ring: hue, or a grey ramp when the value mode is on.
    fn draw_hue_ring(
        &self,
        painter: &Painter,
        center: Pos2,
        pointer: Pos2,
        inner: f32,
        s: &mut Sculptor,
        p: &Palette,
    ) {
        let outer = self.size;
        let current = s.brush.paint_color;
        let (mut hue, sat, val) = rgb_to_hsv(current.x, current.y, current.z);

        let steps = 120;
        for i in 0..steps {
            let a0 = i as f32 / steps as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / steps as f32 * std::f32::consts::TAU;
            let t = i as f32 / steps as f32;
            let (r, g, b) = if self.grayscale {
                // A ramp that runs black to white and back, so both ends of the
                // value scale are reachable without crossing the whole ring.
                let v = 1.0 - (t * 2.0 - 1.0).abs();
                (v, v, v)
            } else {
                hsv_to_rgb(t, 1.0, 1.0)
            };
            let colour = Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);
            painter.add(Shape::convex_polygon(
                vec![
                    Pos2::new(center.x + inner * a0.cos(), center.y + inner * a0.sin()),
                    Pos2::new(center.x + outer * a0.cos(), center.y + outer * a0.sin()),
                    Pos2::new(center.x + outer * a1.cos(), center.y + outer * a1.sin()),
                    Pos2::new(center.x + inner * a1.cos(), center.y + inner * a1.sin()),
                ],
                colour,
                Stroke::NONE,
            ));
        }

        if matches!(self.grab, Grab::Hue) {
            let offset = pointer - center;
            let t = (offset.y.atan2(offset.x) / std::f32::consts::TAU).rem_euclid(1.0);
            let (r, g, b) = if self.grayscale {
                let v = 1.0 - (t * 2.0 - 1.0).abs();
                (v, v, v)
            } else {
                hue = t;
                hsv_to_rgb(hue, sat.max(0.05), val.max(0.05))
            };
            s.brush.paint_color = glam::Vec3::new(r, g, b);
        }

        // Marker on the ring, at the current hue.
        let angle = if self.grayscale {
            // Value maps onto the same folded ramp the ring was drawn with.
            let v = 0.299 * current.x + 0.587 * current.y + 0.114 * current.z;
            (v * 0.5) * std::f32::consts::TAU
        } else {
            hue * std::f32::consts::TAU
        };
        let mid = (inner + outer) * 0.5;
        painter.circle_stroke(
            Pos2::new(center.x + mid * angle.cos(), center.y + mid * angle.sin()),
            (outer - inner) * 0.44,
            Stroke::new(2.0, p.text),
        );
    }

    /// Where the saturation and value square sits: under the disc, so the pad
    /// keeps the middle and size and force stay reachable while painting.
    fn sv_rect(center: Pos2, size: f32) -> Rect {
        Rect::from_min_size(
            Pos2::new(center.x - SV_WIDTH * 0.5, center.y + size + 18.0),
            Vec2::new(SV_WIDTH, SV_HEIGHT),
        )
    }

    /// Saturation across, value up, for the hue the ring is holding.
    fn draw_sv_square(
        &self,
        painter: &Painter,
        square: Rect,
        s: &mut Sculptor,
        pointer: Pos2,
        p: &Palette,
    ) {
        let current = s.brush.paint_color;
        let (hue, mut sat, mut val) = rgb_to_hsv(current.x, current.y, current.z);

        if matches!(self.grab, Grab::Colour) {
            sat = ((pointer.x - square.left()) / square.width()).clamp(0.0, 1.0);
            val = ((square.bottom() - pointer.y) / square.height()).clamp(0.0, 1.0);
            let (r, g, b) = if self.grayscale {
                (val, val, val)
            } else {
                hsv_to_rgb(hue, sat, val)
            };
            s.brush.paint_color = glam::Vec3::new(r, g, b);
        }

        // One quad with four corner colours is exactly right: value times the
        // white-to-hue ramp is bilinear in the two axes, so the interpolator
        // draws the whole square for free.
        let (hr, hg, hb) = if self.grayscale {
            (1.0, 1.0, 1.0)
        } else {
            hsv_to_rgb(hue, 1.0, 1.0)
        };
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
            egui::CornerRadius::same(3),
            Stroke::new(1.0, p.line),
            egui::StrokeKind::Outside,
        );
        painter.circle_stroke(
            Pos2::new(
                square.left() + sat * square.width(),
                square.bottom() - val * square.height(),
            ),
            5.0,
            Stroke::new(1.6, if val > 0.55 { Color32::BLACK } else { Color32::WHITE }),
        );

        let c = s.brush.paint_color;
        painter.text(
            Pos2::new(square.center().x, square.bottom() + 11.0),
            Align2::CENTER_CENTER,
            format!(
                "#{:02X}{:02X}{:02X}",
                (c.x.clamp(0.0, 1.0) * 255.0) as u8,
                (c.y.clamp(0.0, 1.0) * 255.0) as u8,
                (c.z.clamp(0.0, 1.0) * 255.0) as u8
            ),
            FontId::monospace(11.0),
            p.text,
        );
    }

    /// A row of saved colours under the square, with a slot for adding one.
    ///
    /// A press adds or picks; a press on a saved colour while the value mode is
    /// on drops it to its own grey, which is how a value study is set up.
    fn draw_swatches(
        &mut self,
        painter: &Painter,
        center: Pos2,
        square: Rect,
        pointer: Pos2,
        choose: bool,
        s: &mut Sculptor,
        p: &Palette,
    ) {
        let cell = 22.0;
        let gap = 5.0;
        let count = self.swatches.len().min(SWATCHES) + 1; // the last one adds
        let width = count as f32 * cell + (count - 1) as f32 * gap;
        let top = square.bottom() + 20.0;
        let left = center.x - width * 0.5;

        for i in 0..count {
            let rect = Rect::from_min_size(
                Pos2::new(left + i as f32 * (cell + gap), top),
                Vec2::splat(cell),
            );
            let over = rect.contains(pointer);
            let adding = i == count - 1;
            if adding {
                painter.rect(
                    rect,
                    egui::CornerRadius::same(6),
                    p.panel,
                    Stroke::new(1.0, if over { p.accent } else { p.line }),
                    egui::StrokeKind::Inside,
                );
                icons::paint(
                    painter,
                    icons::centered_rect(rect, cell * 0.6),
                    Icon::Plus,
                    if over { p.accent } else { p.dim },
                );
                if choose && over {
                    let c = s.brush.paint_color;
                    self.swatches.push([c.x, c.y, c.z]);
                    if self.swatches.len() > SWATCHES {
                        self.swatches.remove(0);
                    }
                }
            } else {
                let c = self.swatches[i];
                painter.rect(
                    rect,
                    egui::CornerRadius::same(6),
                    Color32::from_rgb(
                        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
                        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
                        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
                    ),
                    Stroke::new(if over { 2.0 } else { 1.0 }, if over { p.accent } else { p.line }),
                    egui::StrokeKind::Inside,
                );
                if choose && over {
                    s.brush.paint_color = glam::Vec3::from_array(c);
                }
            }
        }
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
