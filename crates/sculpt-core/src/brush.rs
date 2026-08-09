//! Sculpting brushes.
//!
//! Every brush runs in two phases: displacements are computed in parallel from
//! an immutable view of the mesh, then written back serially. Reading and
//! writing in one pass would make neighbour-dependent brushes (Smooth) depend
//! on evaluation order.

use crate::mesh::Mesh;
use crate::query;
use glam::Vec3;
use rayon::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BrushKind {
    Draw,
    Clay,
    Flatten,
    Smooth,
    Pinch,
    Crease,
    Inflate,
    Move,
    Paint,
    Mask,
}

impl BrushKind {
    pub const ALL: [BrushKind; 10] = [
        BrushKind::Draw,
        BrushKind::Clay,
        BrushKind::Flatten,
        BrushKind::Smooth,
        BrushKind::Pinch,
        BrushKind::Crease,
        BrushKind::Inflate,
        BrushKind::Move,
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
            BrushKind::Paint => "Paint",
            BrushKind::Mask => "Mask",
        }
    }

    /// Brushes that move geometry and therefore need dyntopo + normal updates.
    pub fn deforms(self) -> bool {
        !matches!(self, BrushKind::Paint | BrushKind::Mask)
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
    pub paint_color: Vec3,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            kind: BrushKind::Clay,
            radius: 0.18,
            strength: 0.4,
            negative: false,
            paint_color: Vec3::new(0.85, 0.3, 0.25),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StrokeInput {
    pub point: Vec3,
    pub normal: Vec3,
    /// World-space movement since the previous stroke step, used by Move.
    pub drag: Vec3,
}

impl StrokeInput {
    /// Mirrors the stroke across the object-space YZ plane.
    pub fn mirrored_x(&self) -> Self {
        Self {
            point: Vec3::new(-self.point.x, self.point.y, self.point.z),
            normal: Vec3::new(-self.normal.x, self.normal.y, self.normal.z),
            drag: Vec3::new(-self.drag.x, self.drag.y, self.drag.z),
        }
    }
}

/// Wyvill kernel: smooth, and its derivative vanishes at both ends so repeated
/// dabs do not leave a visible rim at the brush boundary.
#[inline]
fn falloff(t: f32) -> f32 {
    let s = (1.0 - t * t).max(0.0);
    s * s * s
}

/// Applies one brush dab. Returns the vertices it touched.
pub fn apply(mesh: &mut Mesh, b: &Brush, input: &StrokeInput) -> Vec<u32> {
    let verts = query::verts_in_sphere(mesh, input.point, b.radius);
    if verts.is_empty() {
        return verts;
    }

    let sign = if b.negative { -1.0 } else { 1.0 };
    let inv_r = 1.0 / b.radius.max(1e-6);
    let amp = b.strength * b.radius * 0.25 * sign;

    // Falloff, pre-multiplied by the protection mask.
    let weights: Vec<f32> = verts
        .par_iter()
        .map(|&v| {
            let vx = &mesh.verts[v as usize];
            let t = vx.pos.distance(input.point) * inv_r;
            falloff(t) * (1.0 - vx.mask).clamp(0.0, 1.0)
        })
        .collect();

    match b.kind {
        BrushKind::Paint => {
            for (&v, &w) in verts.iter().zip(&weights) {
                let vx = &mut mesh.verts[v as usize];
                vx.col = vx.col.lerp(b.paint_color, (w * b.strength).clamp(0.0, 1.0));
            }
            return verts;
        }
        BrushKind::Mask => {
            for (&v, &w) in verts.iter().zip(&weights) {
                let vx = &mut mesh.verts[v as usize];
                let d = w * b.strength * 0.5 * sign;
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
                BrushKind::Move => input.drag * (w * b.strength),
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
                    (mean - p) * (w * b.strength)
                }
                BrushKind::Flatten => {
                    let d = (p - plane_p).dot(plane_n);
                    -plane_n * (d * w * b.strength)
                }
                BrushKind::Clay => {
                    // Push toward a plane offset along the surface normal, which
                    // is what gives clay its build-up feel instead of a dent.
                    let d = (p - plane_p).dot(plane_n);
                    plane_n * ((amp - d) * w * b.strength)
                }
                BrushKind::Pinch => {
                    let to_axis = input.point - p;
                    let tangent = to_axis - input.normal * to_axis.dot(input.normal);
                    tangent * (w * b.strength * 0.5)
                }
                BrushKind::Crease => {
                    let to_axis = input.point - p;
                    let tangent = to_axis - input.normal * to_axis.dot(input.normal);
                    tangent * (w * b.strength * 0.6) + input.normal * (amp * w * 0.7)
                }
                BrushKind::Paint | BrushKind::Mask => Vec3::ZERO,
            }
        })
        .collect();

    for (&v, d) in verts.iter().zip(displacements) {
        mesh.verts[v as usize].pos += d;
    }
    verts
}
