//! Transform gizmo: move, rotate and scale the active object.
//!
//! The handles are drawn as 2D shapes projected from the object's axes rather
//! than as 3D geometry, which keeps them a constant size on screen, always on
//! top, and hit-testable without a second render pass. Dragging solves in 3D:
//! move projects the cursor ray onto the axis, rotate reads the angle swept
//! around the projected origin, and scale reads the travel along the projected
//! axis.

use crate::camera::Camera;
use crate::theme::Palette;
use egui::{Color32, Painter, Pos2, Stroke, Vec2};
use glam::{Mat3, Quat, Vec3};
use sculpt_core::Transform;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GizmoMode {
    Off,
    Move,
    Rotate,
    Scale,
}

impl GizmoMode {
    pub const ALL: [GizmoMode; 4] = [
        GizmoMode::Off,
        GizmoMode::Move,
        GizmoMode::Rotate,
        GizmoMode::Scale,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GizmoMode::Off => "Off",
            GizmoMode::Move => "Move",
            GizmoMode::Rotate => "Rotate",
            GizmoMode::Scale => "Scale",
        }
    }
}

/// Which handle is involved. `Screen` is the free handle at the centre.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    X,
    Y,
    Z,
    Screen,
}

impl Handle {
    const AXES: [Handle; 3] = [Handle::X, Handle::Y, Handle::Z];

    fn index(self) -> usize {
        match self {
            Handle::X => 0,
            Handle::Y => 1,
            Handle::Z => 2,
            Handle::Screen => 3,
        }
    }

    fn color(self, p: &Palette) -> Color32 {
        match self {
            Handle::X => Color32::from_rgb(232, 84, 96),
            Handle::Y => Color32::from_rgb(120, 214, 108),
            Handle::Z => Color32::from_rgb(88, 150, 246),
            Handle::Screen => p.text,
        }
    }
}

/// Live drag state.
struct Drag {
    handle: Handle,
    /// Transform as it was when the drag started.
    start: Transform,
    /// Cursor position when the drag started, in points.
    start_cursor: Pos2,
    /// For move: the point on the axis the ray first hit.
    start_anchor: Vec3,
    /// For rotate: the angle of the cursor around the origin at the start.
    start_angle: f32,
}

pub struct Gizmo {
    pub mode: GizmoMode,
    /// Axes follow the object's own rotation rather than the world.
    pub local_space: bool,
    /// Length of the handles, in points.
    pub size: f32,
    drag: Option<Drag>,
    /// Handle under the cursor, refreshed while drawing.
    hover: Option<Handle>,
}

impl Default for Gizmo {
    fn default() -> Self {
        Self {
            mode: GizmoMode::Off,
            local_space: false,
            size: 92.0,
            drag: None,
            hover: None,
        }
    }
}

/// Everything the gizmo needs to know about the frame it is drawn in.
///
/// The viewport is measured in physical pixels and the interface in points, so
/// this is also where the two are reconciled.
#[derive(Clone, Copy)]
pub struct View<'a> {
    pub camera: &'a Camera,
    /// Viewport size in physical pixels.
    pub size: (f32, f32),
    pub pixels_per_point: f32,
}

impl View<'_> {
    /// Projects a world point to interface points, or `None` when behind.
    fn project(&self, p: Vec3) -> Option<Pos2> {
        let clip = self.camera.view_proj(self.size.0 / self.size.1.max(1.0))
            * glam::Vec4::new(p.x, p.y, p.z, 1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        let ppp = self.pixels_per_point.max(1e-3);
        Some(Pos2::new(
            (ndc.x * 0.5 + 0.5) * self.size.0 / ppp,
            (0.5 - ndc.y * 0.5) * self.size.1 / ppp,
        ))
    }

    /// Cursor in points to a world ray.
    fn ray(&self, cursor: Pos2) -> (Vec3, Vec3) {
        let ppp = self.pixels_per_point.max(1e-3);
        self.camera
            .ray(cursor.x * ppp, cursor.y * ppp, self.size.0, self.size.1)
    }
}

impl Gizmo {
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn is_active(&self) -> bool {
        self.mode != GizmoMode::Off
    }

    /// The three axis directions the handles follow.
    fn axes(&self, t: &Transform) -> [Vec3; 3] {
        if self.local_space {
            let m = Mat3::from_quat(t.rotation);
            [m.x_axis, m.y_axis, m.z_axis]
        } else {
            [Vec3::X, Vec3::Y, Vec3::Z]
        }
    }

    /// World-space length of one handle, chosen so it keeps a constant size on
    /// screen whatever the zoom.
    fn handle_length(&self, view: &View, origin: Vec3) -> f32 {
        let per_world = view.camera.pixels_per_world(origin, view.size.1).max(1e-6);
        let wanted_pixels = self.size * view.pixels_per_point.max(1e-3);
        (wanted_pixels / per_world).max(1e-5)
    }

    /// Tries to grab a handle. Returns true when the gizmo took the click, in
    /// which case the caller must not start a stroke.
    pub fn press(&mut self, cursor: Pos2, view: &View, t: &Transform) -> bool {
        if self.mode == GizmoMode::Off {
            return false;
        }
        let Some(handle) = self.pick(cursor, view, t) else {
            return false;
        };
        let origin = t.position;
        let axes = self.axes(t);
        let (ro, rd) = view.ray(cursor);

        let start_anchor = match handle {
            Handle::Screen => plane_point(ro, rd, origin, -view.camera.forward()).unwrap_or(origin),
            h => {
                let axis = axes[h.index()];
                closest_point_on_axis(ro, rd, origin, axis).unwrap_or(origin)
            }
        };
        let start_angle = view
            .project(origin)
            .map(|o| (cursor.y - o.y).atan2(cursor.x - o.x))
            .unwrap_or(0.0);

        self.drag = Some(Drag {
            handle,
            start: *t,
            start_cursor: cursor,
            start_anchor,
            start_angle,
        });
        true
    }

    /// Applies a drag step. Returns the updated transform.
    pub fn drag(&mut self, cursor: Pos2, view: &View, t: &Transform) -> Option<Transform> {
        let d = self.drag.as_ref()?;
        let origin = d.start.position;
        let axes = self.axes(&d.start);
        let (ro, rd) = view.ray(cursor);
        let mut next = d.start;

        match self.mode {
            GizmoMode::Move => {
                let now = match d.handle {
                    Handle::Screen => plane_point(ro, rd, origin, -view.camera.forward())?,
                    h => closest_point_on_axis(ro, rd, origin, axes[h.index()])?,
                };
                next.position = d.start.position + (now - d.start_anchor);
            }
            GizmoMode::Rotate => {
                let screen_origin = view.project(origin)?;
                let angle_now = (cursor.y - screen_origin.y).atan2(cursor.x - screen_origin.x);
                let mut delta = angle_now - d.start_angle;
                while delta > std::f32::consts::PI {
                    delta -= std::f32::consts::TAU;
                }
                while delta < -std::f32::consts::PI {
                    delta += std::f32::consts::TAU;
                }
                let axis = match d.handle {
                    Handle::Screen => -view.camera.forward(),
                    h => axes[h.index()],
                };
                // Screen angles run clockwise, and an axis pointing away from
                // the camera has to turn the other way to track the cursor.
                let facing = if axis.dot(view.camera.forward()) > 0.0 { 1.0 } else { -1.0 };
                next.rotation = (Quat::from_axis_angle(axis.normalize_or(Vec3::Y), delta * facing)
                    * d.start.rotation)
                    .normalize();
            }
            GizmoMode::Scale => {
                let travel = cursor - d.start_cursor;
                let factor = match d.handle {
                    Handle::Screen => {
                        // Up and to the right grows.
                        1.0 + (travel.x - travel.y) * 0.006
                    }
                    h => {
                        let screen_origin = view.project(origin)?;
                        let len = self.handle_length(view, origin);
                        let tip = view.project(origin + axes[h.index()] * len)?;
                        let dir = (tip - screen_origin).normalized();
                        1.0 + travel.dot(dir) * 0.006
                    }
                };
                let factor = factor.clamp(0.05, 20.0);
                next.scale = match d.handle {
                    Handle::Screen => d.start.scale * factor,
                    h => {
                        let mut s = d.start.scale;
                        s[h.index()] *= factor;
                        s
                    }
                };
                next.scale = next.scale.max(Vec3::splat(0.01));
            }
            GizmoMode::Off => return None,
        }
        let _ = t;
        Some(next)
    }

    pub fn release(&mut self) {
        self.drag = None;
    }

    /// Handle nearest the cursor, within grabbing distance.
    fn pick(&self, cursor: Pos2, view: &View, t: &Transform) -> Option<Handle> {
        let origin = t.position;
        let screen_origin = view.project(origin)?;
        let len = self.handle_length(view, origin);
        let axes = self.axes(t);
        let grab = 11.0;

        // The free handle at the centre wins ties, being the smallest target.
        if self.mode != GizmoMode::Rotate && (cursor - screen_origin).length() < grab {
            return Some(Handle::Screen);
        }

        let mut best: Option<(f32, Handle)> = None;
        match self.mode {
            GizmoMode::Rotate => {
                for h in Handle::AXES {
                    let d = ring_distance(view, origin, axes[h.index()], len, cursor)?;
                    if d < grab && best.as_ref().is_none_or(|b| d < b.0) {
                        best = Some((d, h));
                    }
                }
                // The outer ring rotates in the view plane.
                let d = ring_distance(view, origin, -view.camera.forward(), len * 1.22, cursor)?;
                if d < grab && best.as_ref().is_none_or(|b| d < b.0) {
                    best = Some((d, Handle::Screen));
                }
            }
            GizmoMode::Move | GizmoMode::Scale => {
                for h in Handle::AXES {
                    let tip = view.project(origin + axes[h.index()] * len)?;
                    let d = segment_distance(cursor, screen_origin, tip);
                    if d < grab && best.as_ref().is_none_or(|b| d < b.0) {
                        best = Some((d, h));
                    }
                }
            }
            GizmoMode::Off => return None,
        }
        best.map(|(_, h)| h)
    }

    /// Draws the gizmo and refreshes which handle is hovered.
    pub fn draw(
        &mut self,
        ui: &egui::Ui,
        painter: &Painter,
        view: &View,
        t: &Transform,
        p: &Palette,
    ) {
        if self.mode == GizmoMode::Off {
            return;
        }
        let cursor = ui.ctx().pointer_latest_pos();
        self.hover = match (&self.drag, cursor) {
            (Some(d), _) => Some(d.handle),
            (None, Some(c)) => self.pick(c, view, t),
            _ => None,
        };

        let origin = t.position;
        let Some(screen_origin) = view.project(origin) else {
            return;
        };
        let len = self.handle_length(view, origin);
        let axes = self.axes(t);

        match self.mode {
            GizmoMode::Rotate => {
                for h in Handle::AXES {
                    self.draw_ring(painter, view, origin, axes[h.index()], len, h, p);
                }
                self.draw_ring(
                    painter,
                    view,
                    origin,
                    -view.camera.forward(),
                    len * 1.22,
                    Handle::Screen,
                    p,
                );
            }
            GizmoMode::Move | GizmoMode::Scale => {
                for h in Handle::AXES {
                    let Some(tip) = view.project(origin + axes[h.index()] * len) else {
                        continue;
                    };
                    let active = self.hover == Some(h);
                    let color = self.tint(h.color(p), active);
                    let width = if active { 3.4 } else { 2.2 };
                    painter.line_segment([screen_origin, tip], Stroke::new(width, color));
                    let dir = (tip - screen_origin).normalized();
                    if self.mode == GizmoMode::Move {
                        // Arrow head.
                        let n = Vec2::new(-dir.y, dir.x);
                        let base = tip - dir * 11.0;
                        painter.add(egui::Shape::convex_polygon(
                            vec![tip, base + n * 5.0, base - n * 5.0],
                            color,
                            Stroke::NONE,
                        ));
                    } else {
                        painter.rect_filled(
                            egui::Rect::from_center_size(tip, Vec2::splat(if active { 11.0 } else { 9.0 })),
                            egui::CornerRadius::same(2),
                            color,
                        );
                    }
                }
                let active = self.hover == Some(Handle::Screen);
                painter.circle(
                    screen_origin,
                    if active { 7.0 } else { 5.0 },
                    self.tint(p.text, active),
                    Stroke::new(1.0, p.bg),
                );
            }
            GizmoMode::Off => {}
        }
    }

    fn tint(&self, color: Color32, active: bool) -> Color32 {
        if active {
            color
        } else {
            color.gamma_multiply(0.78)
        }
    }

    fn draw_ring(
        &self,
        painter: &Painter,
        view: &View,
        origin: Vec3,
        axis: Vec3,
        radius: f32,
        handle: Handle,
        p: &Palette,
    ) {
        let active = self.hover == Some(handle);
        let color = self.tint(handle.color(p), active);
        let width = if active { 3.0 } else { 1.8 };
        let (u, v) = basis(axis);
        // Only the half facing the camera is drawn, which is what makes a
        // rotation gizmo readable instead of a ball of spaghetti.
        let mut run: Vec<Pos2> = Vec::new();
        for i in 0..=64 {
            let a = i as f32 / 64.0 * std::f32::consts::TAU;
            let world = origin + (u * a.cos() + v * a.sin()) * radius;
            let facing = (world - view.camera.eye()).normalize_or(Vec3::Z);
            let towards = facing.dot((world - origin).normalize_or(Vec3::X)) < 0.15;
            match (towards, view.project(world)) {
                (true, Some(sp)) => run.push(sp),
                _ => {
                    if run.len() > 1 {
                        painter.add(egui::Shape::line(std::mem::take(&mut run), Stroke::new(width, color)));
                    } else {
                        run.clear();
                    }
                }
            }
        }
        if run.len() > 1 {
            painter.add(egui::Shape::line(run, Stroke::new(width, color)));
        }
        let _ = p;
    }
}

impl Gizmo {
    /// The handle under a cursor, for callers deciding whether the click
    /// belongs to the gizmo or to the brush.
    pub fn handle_at(&self, cursor: Pos2, view: &View, t: &Transform) -> Option<Handle> {
        if !self.is_active() {
            return None;
        }
        self.pick(cursor, view, t)
    }
}

// ---------------------------------------------------------------------------
// geometry helpers
// ---------------------------------------------------------------------------

/// Two unit vectors spanning the plane normal to `n`.
fn basis(n: Vec3) -> (Vec3, Vec3) {
    let n = n.normalize_or(Vec3::Y);
    let helper = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = n.cross(helper).normalize_or(Vec3::X);
    (u, n.cross(u))
}

/// Point on the line `origin + axis * s` closest to the ray.
fn closest_point_on_axis(ro: Vec3, rd: Vec3, origin: Vec3, axis: Vec3) -> Option<Vec3> {
    let a = axis.normalize_or(Vec3::X);
    let w = ro - origin;
    let (b, d, e) = (a.dot(rd), a.dot(w), rd.dot(w));
    let denom = 1.0 - b * b;
    if denom.abs() < 1e-5 {
        return None; // the axis points straight at the camera
    }
    Some(origin + a * ((b * e - d) / denom))
}

/// Ray against the plane through `p` with normal `n`.
fn plane_point(ro: Vec3, rd: Vec3, p: Vec3, n: Vec3) -> Option<Vec3> {
    let denom = rd.dot(n);
    if denom.abs() < 1e-6 {
        return None;
    }
    Some(ro + rd * ((p - ro).dot(n) / denom))
}

/// Distance in points from the cursor to a projected ring.
fn ring_distance(view: &View, origin: Vec3, axis: Vec3, radius: f32, cursor: Pos2) -> Option<f32> {
    let (u, v) = basis(axis);
    let mut best = f32::MAX;
    let mut previous: Option<Pos2> = None;
    for i in 0..=48 {
        let a = i as f32 / 48.0 * std::f32::consts::TAU;
        let world = origin + (u * a.cos() + v * a.sin()) * radius;
        let Some(sp) = view.project(world) else {
            previous = None;
            continue;
        };
        if let Some(prev) = previous {
            best = best.min(segment_distance(cursor, prev, sp));
        }
        previous = Some(sp);
    }
    (best < f32::MAX).then_some(best)
}

/// Distance from a point to a segment, in 2D.
fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}
