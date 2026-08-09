//! Sculpting brushes.
//!
//! Every brush runs in two phases: displacements are computed in parallel from
//! an immutable view of the mesh, then written back serially. Reading and
//! writing in one pass would make neighbour-dependent brushes (Smooth) depend
//! on evaluation order.

use crate::mesh::Mesh;
use crate::query;
use glam::{Quat, Vec2, Vec3};
use rayon::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum BrushKind {
    Draw,
    /// The tool a fresh session opens with.
    #[default]
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
    Mask,
}

impl BrushKind {
    pub const ALL: [BrushKind; 13] = [
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
        BrushKind::Paint,
        BrushKind::Mask,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BrushKind::Draw => "Draw",
            BrushKind::Clay => "Clay",
            BrushKind::Flatten => "Flatten",
            BrushKind::Smooth => "Smooth",
            BrushKind::Pinch => "Pinch",
            BrushKind::Crease => "Crease",
            BrushKind::Inflate => "Inflate",
            BrushKind::Move => "Move",
            BrushKind::Drag => "Drag",
            BrushKind::Twist => "Twist",
            BrushKind::Scale => "Scale",
            BrushKind::Paint => "Paint",
            BrushKind::Mask => "Mask",
        }
    }

    /// One-line hint shown in the tooltip.
    pub fn hint(self) -> &'static str {
        match self {
            BrushKind::Draw => "Push the surface along its normal",
            BrushKind::Clay => "Build up material toward a local plane",
            BrushKind::Flatten => "Pull the surface onto its average plane",
            BrushKind::Smooth => "Relax the surface toward its neighbours",
            BrushKind::Pinch => "Draw the surface toward the cursor",
            BrushKind::Crease => "Pinch and push, for sharp folds",
            BrushKind::Inflate => "Expand along per-vertex normals",
            BrushKind::Move => "Grab and move a region",
            BrushKind::Drag => "Drag the surface with the cursor",
            BrushKind::Twist => "Rotate a region around the view axis",
            BrushKind::Scale => "Grow or shrink a region",
            BrushKind::Paint => "Paint colour and material",
            BrushKind::Mask => "Protect a region from sculpting",
        }
    }

    /// Brushes that move geometry and therefore need dyntopo + normal updates.
    pub fn deforms(self) -> bool {
        !matches!(self, BrushKind::Paint | BrushKind::Mask)
    }

    /// Brushes driven by cursor motion rather than by surface position; these
    /// keep their grab point instead of re-picking every step.
    pub fn is_grab(self) -> bool {
        matches!(
            self,
            BrushKind::Move | BrushKind::Drag | BrushKind::Twist | BrushKind::Scale
        )
    }

    /// Brushes for which a negative pass is meaningful.
    pub fn has_negative(self) -> bool {
        !matches!(self, BrushKind::Move | BrushKind::Drag | BrushKind::Paint)
    }
}

/// Radial weighting inside the brush disc.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Falloff {
    Smooth,
    Linear,
    Sharp,
    Sphere,
    Constant,
}

impl Falloff {
    pub const ALL: [Falloff; 5] = [
        Falloff::Smooth,
        Falloff::Linear,
        Falloff::Sharp,
        Falloff::Sphere,
        Falloff::Constant,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Falloff::Smooth => "Smooth",
            Falloff::Linear => "Linear",
            Falloff::Sharp => "Sharp",
            Falloff::Sphere => "Sphere",
            Falloff::Constant => "Constant",
        }
    }

    /// `t` is the normalised distance from the brush centre, 0 at the middle.
    #[inline]
    pub fn eval(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            // Wyvill kernel: its derivative vanishes at both ends, so repeated
            // dabs leave no visible rim at the brush boundary.
            Falloff::Smooth => {
                let s = 1.0 - t * t;
                s * s * s
            }
            Falloff::Linear => 1.0 - t,
            Falloff::Sharp => (1.0 - t) * (1.0 - t),
            Falloff::Sphere => (1.0 - t * t).max(0.0).sqrt(),
            Falloff::Constant => {
                if t >= 1.0 {
                    0.0
                } else {
                    1.0
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Brush {
    pub kind: BrushKind,
    /// World-space radius.
    pub radius: f32,
    /// 0..1.
    pub strength: f32,
    /// Inverts the brush (ctrl in most sculpting apps).
    pub negative: bool,
    pub falloff: Falloff,
    /// Skip vertices facing away from the camera.
    pub culling: bool,
    /// Keep the dab anchored where the stroke started instead of following the
    /// surface, which is how you carve a straight ridge.
    pub lock_plane: bool,
    /// Relax the touched region a little after each dab.
    pub auto_smooth: f32,
    /// Scale radius and strength with tablet pressure.
    pub pressure_radius: bool,
    pub pressure_strength: bool,
    pub paint_color: Vec3,
    pub paint_rough: f32,
    pub paint_metal: f32,
    pub paint_albedo: bool,
    pub paint_material: bool,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            kind: BrushKind::Clay,
            radius: 0.18,
            strength: 0.4,
            negative: false,
            falloff: Falloff::Smooth,
            culling: false,
            lock_plane: false,
            auto_smooth: 0.0,
            pressure_radius: false,
            pressure_strength: true,
            paint_color: Vec3::new(0.85, 0.3, 0.25),
            paint_rough: 0.6,
            paint_metal: 0.0,
            paint_albedo: true,
            paint_material: false,
        }
    }
}

impl Brush {
    /// Sensible per-tool defaults, applied when the tool is first selected.
    pub fn defaults_for(kind: BrushKind) -> Self {
        let mut b = Brush { kind, ..Default::default() };
        match kind {
            BrushKind::Draw => b.strength = 0.35,
            BrushKind::Clay => b.strength = 0.45,
            BrushKind::Flatten => b.strength = 0.5,
            BrushKind::Smooth => b.strength = 0.6,
            BrushKind::Pinch => b.strength = 0.4,
            BrushKind::Crease => {
                b.strength = 0.45;
                b.radius = 0.1;
            }
            BrushKind::Inflate => b.strength = 0.3,
            BrushKind::Move => {
                b.strength = 1.0;
                b.radius = 0.3;
            }
            BrushKind::Drag => {
                b.strength = 1.0;
                b.radius = 0.3;
            }
            BrushKind::Twist => {
                b.strength = 1.0;
                b.radius = 0.3;
            }
            BrushKind::Scale => b.strength = 0.6,
            BrushKind::Paint => b.strength = 0.6,
            BrushKind::Mask => b.strength = 0.7,
        }
        b
    }
}

/// One step of a stroke, already resolved into object space.
#[derive(Clone, Copy, Debug)]
pub struct StrokeInput {
    pub point: Vec3,
    pub normal: Vec3,
    /// World-space movement since the previous step, used by Move and Drag.
    pub drag: Vec3,
    /// Camera forward, pointing into the scene. Drives culling and Twist.
    pub view_dir: Vec3,
    /// Signed cursor rotation around the dab centre, in radians (Twist).
    pub twist: f32,
    /// Signed cursor motion along the screen X axis, normalised (Scale).
    pub pinch: f32,
    /// Tablet or touch pressure, 1.0 when unknown.
    pub pressure: f32,
}

impl Default for StrokeInput {
    fn default() -> Self {
        Self {
            point: Vec3::ZERO,
            normal: Vec3::Y,
            drag: Vec3::ZERO,
            view_dir: -Vec3::Z,
            twist: 0.0,
            pinch: 0.0,
            pressure: 1.0,
        }
    }
}

/// Symmetry plane, expressed as an axis index (0 = X, 1 = Y, 2 = Z).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    pub fn label(self) -> &'static str {
        match self {
            Axis::X => "X",
            Axis::Y => "Y",
            Axis::Z => "Z",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }

    pub fn vec(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }

    /// Mirrors a point across the plane through `origin` normal to this axis.
    #[inline]
    pub fn mirror_point(self, p: Vec3, origin: f32) -> Vec3 {
        let mut q = p;
        let i = self.index();
        q[i] = 2.0 * origin - q[i];
        q
    }

    #[inline]
    pub fn mirror_dir(self, d: Vec3) -> Vec3 {
        let mut q = d;
        q[self.index()] = -q[self.index()];
        q
    }
}

impl StrokeInput {
    /// Mirrors the stroke across a symmetry plane.
    pub fn mirrored(&self, axis: Axis, origin: f32) -> Self {
        Self {
            point: axis.mirror_point(self.point, origin),
            normal: axis.mirror_dir(self.normal),
            drag: axis.mirror_dir(self.drag),
            view_dir: axis.mirror_dir(self.view_dir),
            // Mirroring flips handedness, so the rotation reverses.
            twist: -self.twist,
            pinch: self.pinch,
            pressure: self.pressure,
        }
    }
}

/// Applies one brush dab. Returns the vertices it touched.
pub fn apply(mesh: &mut Mesh, b: &Brush, input: &StrokeInput) -> Vec<u32> {
    let pressure = input.pressure.clamp(0.05, 1.0);
    let radius = if b.pressure_radius { b.radius * pressure } else { b.radius };
    let strength = if b.pressure_strength { b.strength * pressure } else { b.strength };

    let mut verts = query::verts_in_sphere(mesh, input.point, radius);
    if b.culling {
        verts.retain(|&v| mesh.verts[v as usize].nrm.dot(input.view_dir) < 0.0);
    }
    if verts.is_empty() {
        return verts;
    }

    let sign = if b.negative && b.kind.has_negative() { -1.0 } else { 1.0 };
    let inv_r = 1.0 / radius.max(1e-6);
    let amp = strength * radius * 0.25 * sign;

    // Falloff, pre-multiplied by the protection mask.
    let weights: Vec<f32> = verts
        .par_iter()
        .map(|&v| {
            let vx = &mesh.verts[v as usize];
            let t = vx.pos.distance(input.point) * inv_r;
            let m = if b.kind == BrushKind::Mask {
                1.0
            } else {
                (1.0 - vx.mask).clamp(0.0, 1.0)
            };
            b.falloff.eval(t) * m
        })
        .collect();

    match b.kind {
        BrushKind::Paint => {
            for (&v, &w) in verts.iter().zip(&weights) {
                let vx = &mut mesh.verts[v as usize];
                let t = (w * strength).clamp(0.0, 1.0);
                if b.paint_albedo {
                    vx.col = vx.col.lerp(b.paint_color, t);
                }
                if b.paint_material {
                    vx.rough += (b.paint_rough - vx.rough) * t;
                    vx.metal += (b.paint_metal - vx.metal) * t;
                }
            }
            return verts;
        }
        BrushKind::Mask => {
            for (&v, &w) in verts.iter().zip(&weights) {
                let vx = &mut mesh.verts[v as usize];
                let d = w * strength * 0.5 * sign;
                vx.mask = (vx.mask + d).clamp(0.0, 1.0);
            }
            return verts;
        }
        _ => {}
    }

    // Area-weighted plane through the dab, used by the plane-relative brushes.
    let (plane_p, plane_n) = if matches!(b.kind, BrushKind::Clay | BrushKind::Flatten) {
        let mut sw = 0.0;
        let mut sp = Vec3::ZERO;
        let mut sn = Vec3::ZERO;
        for (&v, &w) in verts.iter().zip(&weights) {
            let vx = &mesh.verts[v as usize];
            sw += w;
            sp += vx.pos * w;
            sn += vx.nrm * w;
        }
        if sw > 1e-6 {
            (sp / sw, sn.normalize_or(input.normal))
        } else {
            (input.point, input.normal)
        }
    } else {
        (input.point, input.normal)
    };

    let twist_rot = (b.kind == BrushKind::Twist).then(|| {
        let axis = (-input.view_dir).normalize_or(Vec3::Z);
        (axis, input.twist * strength)
    });

    let displacements: Vec<Vec3> = verts
        .par_iter()
        .zip(weights.par_iter())
        .map(|(&v, &w)| {
            if w <= 0.0 {
                return Vec3::ZERO;
            }
            let vx = &mesh.verts[v as usize];
            let p = vx.pos;
            match b.kind {
                BrushKind::Draw => input.normal * (amp * w),
                BrushKind::Inflate => vx.nrm * (amp * w),
                BrushKind::Move | BrushKind::Drag => input.drag * (w * strength),
                BrushKind::Smooth => {
                    let nb = mesh.neighbors(v);
                    if nb.is_empty() {
                        return Vec3::ZERO;
                    }
                    let mut mean = Vec3::ZERO;
                    for &n in &nb {
                        mean += mesh.verts[n as usize].pos;
                    }
                    mean /= nb.len() as f32;
                    let mut delta = mean - p;
                    if b.negative {
                        // Inverted smooth sharpens: push away from the mean.
                        delta = -delta;
                    }
                    delta * (w * strength)
                }
                BrushKind::Flatten => {
                    let d = (p - plane_p).dot(plane_n);
                    -plane_n * (d * w * strength)
                }
                BrushKind::Clay => {
                    // Push toward a plane offset along the surface normal, which
                    // is what gives clay its build-up feel instead of a dent.
                    let d = (p - plane_p).dot(plane_n);
                    plane_n * ((amp - d) * w * strength)
                }
                BrushKind::Pinch => {
                    let to_axis = input.point - p;
                    let tangent = to_axis - input.normal * to_axis.dot(input.normal);
                    tangent * (w * strength * 0.5 * sign)
                }
                BrushKind::Crease => {
                    let to_axis = input.point - p;
                    let tangent = to_axis - input.normal * to_axis.dot(input.normal);
                    tangent * (w * strength * 0.6) + input.normal * (amp * w * 0.7)
                }
                BrushKind::Twist => {
                    let (axis, angle) = twist_rot.unwrap_or((Vec3::Z, 0.0));
                    if angle.abs() < 1e-6 {
                        return Vec3::ZERO;
                    }
                    let q = Quat::from_axis_angle(axis, angle * w);
                    let local = p - input.point;
                    q * local - local
                }
                BrushKind::Scale => (p - input.point) * (w * strength * input.pinch),
                BrushKind::Paint | BrushKind::Mask => Vec3::ZERO,
            }
        })
        .collect();

    for (&v, d) in verts.iter().zip(displacements) {
        mesh.verts[v as usize].pos += d;
    }

    if b.auto_smooth > 0.0 {
        relax(mesh, &verts, &weights, b.auto_smooth * strength);
    }

    mesh.commit_moves(&verts);
    verts
}

/// Laplacian relaxation of a vertex set, weighted per vertex.
pub fn relax(mesh: &mut Mesh, verts: &[u32], weights: &[f32], amount: f32) {
    let deltas: Vec<Vec3> = verts
        .par_iter()
        .zip(weights.par_iter())
        .map(|(&v, &w)| {
            let nb = mesh.neighbors(v);
            if nb.is_empty() {
                return Vec3::ZERO;
            }
            let mut mean = Vec3::ZERO;
            for &n in &nb {
                mean += mesh.verts[n as usize].pos;
            }
            mean /= nb.len() as f32;
            (mean - mesh.verts[v as usize].pos) * (w * amount).clamp(0.0, 1.0)
        })
        .collect();
    for (&v, d) in verts.iter().zip(deltas) {
        mesh.verts[v as usize].pos += d;
    }
}

/// Screen-space helpers the app uses to build a [`StrokeInput`].
pub fn signed_angle_2d(a: Vec2, b: Vec2) -> f32 {
    let dot = a.x * b.x + a.y * b.y;
    let det = a.x * b.y - a.y * b.x;
    det.atan2(dot)
}
