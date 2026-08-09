//! Vector icons, painted with egui shapes.
//!
//! Drawing them in code rather than shipping an icon font keeps the binary
//! asset-free and lets every glyph scale cleanly to whatever the touch target
//! size ends up being. Each icon is authored in a unit square and mapped onto
//! the rect it is asked to fill.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use sculpt_core::BrushKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    // Brushes
    Draw,
    Clay,
    Flatten,
    Smooth,
    Pinch,
    Crease,
    Inflate,
    Move,
    Drag,
    Twist,
    Scale,
    Paint,
    Smudge,
    ColorBlur,
    Fill,
    Mask,
    // Actions
    Undo,
    Redo,
    Symmetry,
    Wireframe,
    Grid,
    FrameView,
    Open,
    Save,
    New,
    Subdivide,
    Remesh,
    Decimate,
    CloseHoles,
    Mirror,
    Duplicate,
    Trash,
    Eye,
    EyeOff,
    Plus,
    Settings,
    Camera,
    Palette,
    Menu,
    Close,
    Pin,
    Reset,
}

impl Icon {
    pub fn of_brush(kind: BrushKind) -> Self {
        match kind {
            BrushKind::Draw => Icon::Draw,
            BrushKind::Clay => Icon::Clay,
            BrushKind::Flatten => Icon::Flatten,
            BrushKind::Smooth => Icon::Smooth,
            BrushKind::Pinch => Icon::Pinch,
            BrushKind::Crease => Icon::Crease,
            BrushKind::Inflate => Icon::Inflate,
            BrushKind::Move => Icon::Move,
            BrushKind::Drag => Icon::Drag,
            BrushKind::Twist => Icon::Twist,
            BrushKind::Scale => Icon::Scale,
            BrushKind::Paint => Icon::Paint,
            BrushKind::Smudge => Icon::Smudge,
            BrushKind::ColorBlur => Icon::ColorBlur,
            BrushKind::Fill => Icon::Fill,
            BrushKind::Mask => Icon::Mask,
        }
    }
}

/// Maps unit-square coordinates onto the target rect, with a little padding so
/// strokes never touch the edge.
struct Pen<'a> {
    painter: &'a Painter,
    rect: Rect,
    stroke: Stroke,
}

impl<'a> Pen<'a> {
    fn new(painter: &'a Painter, rect: Rect, color: Color32, width: f32) -> Self {
        let pad = rect.width() * 0.14;
        Self {
            painter,
            rect: rect.shrink(pad),
            stroke: Stroke::new(width, color),
        }
    }

    #[inline]
    fn p(&self, x: f32, y: f32) -> Pos2 {
        Pos2::new(
            self.rect.left() + x * self.rect.width(),
            self.rect.top() + y * self.rect.height(),
        )
    }

    fn line(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter.add(Shape::line(v, self.stroke));
    }

    fn closed(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter.add(Shape::closed_line(v, self.stroke));
    }

    fn filled(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.painter
            .add(Shape::convex_polygon(v, self.stroke.color, Stroke::NONE));
    }

    fn circle(&self, cx: f32, cy: f32, r: f32) {
        self.painter.add(Shape::circle_stroke(
            self.p(cx, cy),
            r * self.rect.width(),
            self.stroke,
        ));
    }

    fn disc(&self, cx: f32, cy: f32, r: f32) {
        self.painter.add(Shape::circle_filled(
            self.p(cx, cy),
            r * self.rect.width(),
            self.stroke.color,
        ));
    }

    /// Arc from `a0` to `a1` radians, drawn as a polyline.
    fn arc(&self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
        let steps = 24;
        let pts: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let t = a0 + (a1 - a0) * i as f32 / steps as f32;
                self.p(cx + r * t.cos(), cy + r * t.sin())
            })
            .collect();
        self.painter.add(Shape::line(pts, self.stroke));
    }

    /// Arrow head at `tip`, pointing along `dir` (unit-square units).
    fn head(&self, tip: (f32, f32), dir: (f32, f32), size: f32) {
        let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1e-6);
        let d = (dir.0 / len, dir.1 / len);
        let n = (-d.1, d.0);
        let base = (tip.0 - d.0 * size, tip.1 - d.1 * size);
        self.filled(&[
            tip,
            (base.0 + n.0 * size * 0.55, base.1 + n.1 * size * 0.55),
            (base.0 - n.0 * size * 0.55, base.1 - n.1 * size * 0.55),
        ]);
    }

    fn arrow(&self, from: (f32, f32), to: (f32, f32), head: f32) {
        self.line(&[from, to]);
        self.head(to, (to.0 - from.0, to.1 - from.1), head);
    }

    /// A sine wave sampled across `x0..x1`.
    fn wave(&self, x0: f32, x1: f32, y: f32, amp: f32, cycles: f32) {
        let steps = 40;
        let pts: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32;
                let x = x0 + (x1 - x0) * t;
                self.p(x, y + amp * (t * cycles * std::f32::consts::TAU).sin())
            })
            .collect();
        self.painter.add(Shape::line(pts, self.stroke));
    }
}

/// Paints `icon` inside `rect`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let width = (rect.width() * 0.075).clamp(1.2, 2.6);
    let k = Pen::new(painter, rect, color, width);
    use Icon::*;
    match icon {
        // ---- brushes -------------------------------------------------------
        Draw => {
            // A bump pushed out of a flat surface.
            k.line(&[(0.0, 0.78), (0.28, 0.78)]);
            k.line(&[(0.72, 0.78), (1.0, 0.78)]);
            k.line(&[
                (0.28, 0.78), (0.34, 0.52), (0.5, 0.42), (0.66, 0.52), (0.72, 0.78),
            ]);
            k.arrow((0.5, 0.3), (0.5, 0.04), 0.13);
        }
        Clay => {
            // Layers building up under a trowel.
            k.line(&[(0.05, 0.86), (0.95, 0.86)]);
            k.line(&[(0.18, 0.66), (0.82, 0.66)]);
            k.line(&[(0.3, 0.46), (0.7, 0.46)]);
            k.arrow((0.5, 0.08), (0.5, 0.34), 0.13);
        }
        Flatten => {
            k.line(&[(0.05, 0.62), (0.95, 0.62)]);
            k.arrow((0.28, 0.12), (0.28, 0.5), 0.11);
            k.arrow((0.72, 0.12), (0.72, 0.5), 0.11);
            k.line(&[(0.12, 0.86), (0.88, 0.86)]);
        }
        Smooth => {
            // Rough on the left, calm on the right.
            k.wave(0.0, 0.55, 0.5, 0.22, 1.6);
            k.line(&[(0.55, 0.5), (1.0, 0.5)]);
        }
        Pinch => {
            // Four arrows converging, which is what pinch does to the surface.
            k.arrow((0.04, 0.22), (0.38, 0.42), 0.17);
            k.arrow((0.96, 0.22), (0.62, 0.42), 0.17);
            k.arrow((0.04, 0.78), (0.38, 0.58), 0.17);
            k.arrow((0.96, 0.78), (0.62, 0.58), 0.17);
        }
        Crease => {
            k.line(&[(0.04, 0.24), (0.5, 0.86), (0.96, 0.24)]);
            k.line(&[(0.28, 0.1), (0.5, 0.42), (0.72, 0.1)]);
        }
        Inflate => {
            k.circle(0.5, 0.5, 0.2);
            // Arrows pushing outward on the diagonals, so it cannot be confused
            // with the four-way Move glyph.
            let d = std::f32::consts::FRAC_1_SQRT_2;
            for (dx, dy) in [(d, -d), (d, d), (-d, d), (-d, -d)] {
                k.arrow(
                    (0.5 + dx * 0.3, 0.5 + dy * 0.3),
                    (0.5 + dx * 0.52, 0.5 + dy * 0.52),
                    0.16,
                );
            }
        }
        Move => {
            k.line(&[(0.5, 0.24), (0.5, 0.76)]);
            k.line(&[(0.24, 0.5), (0.76, 0.5)]);
            k.head((0.5, 0.02), (0.0, -1.0), 0.23);
            k.head((0.5, 0.98), (0.0, 1.0), 0.23);
            k.head((0.02, 0.5), (-1.0, 0.0), 0.23);
            k.head((0.98, 0.5), (1.0, 0.0), 0.23);
        }
        Drag => {
            // A point pulled along, leaving a trail.
            k.disc(0.72, 0.32, 0.1);
            k.line(&[(0.1, 0.82), (0.62, 0.4)]);
            k.line(&[(0.1, 0.6), (0.34, 0.6)]);
            k.line(&[(0.28, 0.86), (0.28, 0.64)]);
        }
        Twist => {
            // Open circular arrow, wound anticlockwise.
            let (r, a0, a1) = (0.36, 0.7, 5.6);
            k.arc(0.5, 0.5, r, a0, a1);
            let tip = (0.5 + r * a1.cos(), 0.5 + r * a1.sin());
            // Tangent at the end of the arc.
            k.head(tip, (-a1.sin(), a1.cos()), 0.2);
            k.disc(0.5, 0.5, 0.08);
        }
        Scale => {
            k.circle(0.5, 0.5, 0.16);
            k.circle(0.5, 0.5, 0.38);
            k.arrow((0.5, 0.34), (0.5, 0.1), 0.1);
            k.arrow((0.5, 0.66), (0.5, 0.9), 0.1);
        }
        Paint => {
            // A loaded brush tip.
            k.filled(&[(0.5, 0.06), (0.72, 0.5), (0.5, 0.72), (0.28, 0.5)]);
            k.line(&[(0.5, 0.72), (0.5, 0.94)]);
            k.line(&[(0.3, 0.94), (0.7, 0.94)]);
        }
        Smudge => {
            // A finger dragging colour, leaving a tapering smear.
            k.line(&[(0.08, 0.86), (0.42, 0.52)]);
            k.line(&[(0.2, 0.9), (0.54, 0.56)]);
            k.disc(0.66, 0.4, 0.13);
            k.arc(0.66, 0.4, 0.26, -2.2, 0.9);
        }
        ColorBlur => {
            // Sharp edge on the left dissolving into a soft one on the right.
            k.line(&[(0.16, 0.1), (0.16, 0.9)]);
            k.line(&[(0.38, 0.16), (0.38, 0.84)]);
            k.line(&[(0.58, 0.24), (0.58, 0.76)]);
            k.line(&[(0.78, 0.34), (0.78, 0.66)]);
            k.disc(0.94, 0.5, 0.05);
        }
        Fill => {
            // A tipped bucket pouring onto a surface.
            k.closed(&[(0.1, 0.3), (0.58, 0.06), (0.72, 0.42), (0.24, 0.66)]);
            k.line(&[(0.62, 0.24), (0.86, 0.34)]);
            k.filled(&[(0.86, 0.42), (0.96, 0.62), (0.76, 0.62)]);
            k.line(&[(0.06, 0.92), (0.94, 0.92)]);
        }
        Mask => {
            // Half-protected disc.
            k.circle(0.5, 0.5, 0.4);
            let pts: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let t = std::f32::consts::FRAC_PI_2
                        + std::f32::consts::PI * i as f32 / 24.0;
                    k.p(0.5 + 0.4 * t.cos(), 0.5 + 0.4 * t.sin())
                })
                .collect();
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }

        // ---- actions -------------------------------------------------------
        Undo => {
            // Arc over the top, with the head landing on the left end.
            let (r, a0, a1) = (0.34f32, std::f32::consts::PI + 0.35, std::f32::consts::TAU + 0.5);
            k.arc(0.5, 0.62, r, a0, a1);
            let tip = (0.5 + r * a0.cos(), 0.62 + r * a0.sin());
            k.head(tip, (a0.sin(), -a0.cos()), 0.2);
        }
        Redo => {
            let (r, a0, a1) = (0.34f32, -0.5, std::f32::consts::PI - 0.35);
            k.arc(0.5, 0.62, r, a0, a1);
            let tip = (0.5 + r * a1.cos(), 0.62 + r * a1.sin());
            k.head(tip, (-a1.sin(), a1.cos()), 0.2);
        }
        Symmetry => {
            k.line(&[(0.5, 0.04), (0.5, 0.96)]);
            k.closed(&[(0.06, 0.24), (0.38, 0.5), (0.06, 0.76)]);
            k.filled(&[(0.94, 0.24), (0.62, 0.5), (0.94, 0.76)]);
        }
        Wireframe => {
            k.closed(&[(0.5, 0.06), (0.96, 0.32), (0.96, 0.72), (0.5, 0.96), (0.04, 0.72), (0.04, 0.32)]);
            k.line(&[(0.5, 0.06), (0.5, 0.5)]);
            k.line(&[(0.5, 0.5), (0.96, 0.72)]);
            k.line(&[(0.5, 0.5), (0.04, 0.72)]);
        }
        Grid => {
            for i in 0..3 {
                let t = 0.25 + i as f32 * 0.25;
                k.line(&[(t, 0.08), (t, 0.92)]);
                k.line(&[(0.08, t), (0.92, t)]);
            }
        }
        FrameView => {
            for (sx, sy) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let x = 0.06 + sx * 0.88;
                let y = 0.06 + sy * 0.88;
                let dx = if sx == 0.0 { 0.22 } else { -0.22 };
                let dy = if sy == 0.0 { 0.22 } else { -0.22 };
                k.line(&[(x, y), (x + dx, y)]);
                k.line(&[(x, y), (x, y + dy)]);
            }
            k.disc(0.5, 0.5, 0.12);
        }
        Open => {
            k.line(&[(0.06, 0.8), (0.06, 0.24), (0.4, 0.24), (0.5, 0.36), (0.94, 0.36)]);
            k.line(&[(0.06, 0.8), (0.94, 0.8), (0.94, 0.36)]);
            k.arrow((0.5, 0.72), (0.5, 0.46), 0.11);
        }
        Save => {
            k.closed(&[(0.08, 0.1), (0.78, 0.1), (0.92, 0.26), (0.92, 0.9), (0.08, 0.9)]);
            k.line(&[(0.28, 0.1), (0.28, 0.42), (0.7, 0.42), (0.7, 0.1)]);
            k.line(&[(0.28, 0.9), (0.28, 0.62), (0.72, 0.62), (0.72, 0.9)]);
        }
        New => {
            k.circle(0.5, 0.5, 0.4);
            k.line(&[(0.5, 0.26), (0.5, 0.74)]);
            k.line(&[(0.26, 0.5), (0.74, 0.5)]);
        }
        Subdivide => {
            k.closed(&[(0.5, 0.06), (0.96, 0.9), (0.04, 0.9)]);
            k.line(&[(0.27, 0.48), (0.73, 0.48)]);
            k.line(&[(0.27, 0.48), (0.5, 0.9)]);
            k.line(&[(0.73, 0.48), (0.5, 0.9)]);
        }
        Remesh => {
            k.closed(&[(0.1, 0.28), (0.5, 0.08), (0.9, 0.28), (0.9, 0.72), (0.5, 0.92), (0.1, 0.72)]);
            k.line(&[(0.1, 0.28), (0.5, 0.5), (0.9, 0.28)]);
            k.line(&[(0.5, 0.5), (0.5, 0.92)]);
        }
        Decimate => {
            k.closed(&[(0.5, 0.08), (0.92, 0.86), (0.08, 0.86)]);
            k.arrow((0.28, 0.4), (0.5, 0.62), 0.1);
            k.arrow((0.72, 0.4), (0.5, 0.62), 0.1);
        }
        CloseHoles => {
            k.circle(0.5, 0.5, 0.4);
            k.arc(0.5, 0.5, 0.18, 0.0, std::f32::consts::TAU);
            k.line(&[(0.5, 0.32), (0.5, 0.68)]);
            k.line(&[(0.32, 0.5), (0.68, 0.5)]);
        }
        Mirror => {
            k.line(&[(0.5, 0.06), (0.5, 0.94)]);
            k.closed(&[(0.08, 0.3), (0.38, 0.3), (0.38, 0.7), (0.08, 0.7)]);
            k.filled(&[(0.62, 0.3), (0.92, 0.3), (0.92, 0.7), (0.62, 0.7)]);
        }
        Duplicate => {
            k.closed(&[(0.06, 0.26), (0.62, 0.26), (0.62, 0.94), (0.06, 0.94)]);
            k.closed(&[(0.36, 0.06), (0.94, 0.06), (0.94, 0.72), (0.7, 0.72)]);
        }
        Trash => {
            k.line(&[(0.1, 0.24), (0.9, 0.24)]);
            k.line(&[(0.36, 0.24), (0.4, 0.1), (0.6, 0.1), (0.64, 0.24)]);
            k.line(&[(0.2, 0.24), (0.26, 0.94), (0.74, 0.94), (0.8, 0.24)]);
            k.line(&[(0.42, 0.4), (0.44, 0.8)]);
            k.line(&[(0.58, 0.4), (0.56, 0.8)]);
        }
        Eye => {
            k.line(&[(0.04, 0.5), (0.24, 0.24), (0.76, 0.24), (0.96, 0.5)]);
            k.line(&[(0.04, 0.5), (0.24, 0.76), (0.76, 0.76), (0.96, 0.5)]);
            k.disc(0.5, 0.5, 0.15);
        }
        EyeOff => {
            k.line(&[(0.04, 0.5), (0.24, 0.24), (0.76, 0.24), (0.96, 0.5)]);
            k.line(&[(0.04, 0.5), (0.24, 0.76), (0.76, 0.76), (0.96, 0.5)]);
            k.circle(0.5, 0.5, 0.15);
            k.line(&[(0.1, 0.9), (0.9, 0.1)]);
        }
        Plus => {
            k.line(&[(0.5, 0.12), (0.5, 0.88)]);
            k.line(&[(0.12, 0.5), (0.88, 0.5)]);
        }
        Settings => {
            k.circle(0.5, 0.5, 0.2);
            for i in 0..6 {
                let a = i as f32 * std::f32::consts::TAU / 6.0;
                k.line(&[
                    (0.5 + 0.3 * a.cos(), 0.5 + 0.3 * a.sin()),
                    (0.5 + 0.46 * a.cos(), 0.5 + 0.46 * a.sin()),
                ]);
            }
        }
        Camera => {
            k.closed(&[(0.06, 0.28), (0.3, 0.28), (0.38, 0.14), (0.62, 0.14), (0.7, 0.28), (0.94, 0.28), (0.94, 0.88), (0.06, 0.88)]);
            k.circle(0.5, 0.56, 0.18);
        }
        Palette => {
            k.arc(0.5, 0.55, 0.42, std::f32::consts::PI, std::f32::consts::TAU);
            k.line(&[(0.08, 0.55), (0.08, 0.7)]);
            k.line(&[(0.92, 0.55), (0.92, 0.66)]);
            k.disc(0.3, 0.42, 0.07);
            k.disc(0.5, 0.34, 0.07);
            k.disc(0.7, 0.42, 0.07);
        }
        Menu => {
            for i in 0..3 {
                let y = 0.24 + i as f32 * 0.26;
                k.line(&[(0.1, y), (0.9, y)]);
            }
        }
        Close => {
            k.line(&[(0.16, 0.16), (0.84, 0.84)]);
            k.line(&[(0.84, 0.16), (0.16, 0.84)]);
        }
        Pin => {
            k.line(&[(0.5, 0.56), (0.5, 0.96)]);
            k.closed(&[(0.28, 0.12), (0.72, 0.12), (0.62, 0.34), (0.66, 0.56), (0.34, 0.56), (0.38, 0.34)]);
        }
        Reset => {
            k.arc(0.5, 0.5, 0.36, 0.7, std::f32::consts::TAU - 0.2);
            k.head((0.5 + 0.36 * 0.76, 0.5 + 0.36 * 0.64), (-0.6, 0.9), 0.14);
            k.disc(0.5, 0.5, 0.06);
        }
    }
}

/// Convenience for placing an icon centred in a rect of a given size.
pub fn centered_rect(host: Rect, size: f32) -> Rect {
    Rect::from_center_size(host.center(), Vec2::splat(size))
}
