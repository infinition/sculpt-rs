//! Spatial queries.
//!
//! Every entry point tries the mesh's spatial grid first and falls back to a
//! parallel linear scan when there is none, or when the grid decides the query
//! would degenerate (a brush swallowing the whole model, say). The fallback is
//! what the tests exercise, so both paths stay honest.

use crate::mesh::Mesh;
use glam::Vec3;
use rayon::prelude::*;
use rustc_hash::FxHashSet;

/// Indices of every vertex inside the sphere.
pub fn verts_in_sphere(mesh: &Mesh, center: Vec3, radius: f32) -> Vec<u32> {
    if let Some(g) = &mesh.accel {
        if let Some(v) = g.verts_in_sphere(mesh, center, radius) {
            return v;
        }
    }
    let r2 = radius * radius;
    mesh.verts
        .par_iter()
        .enumerate()
        .filter_map(|(i, v)| (v.pos.distance_squared(center) <= r2).then_some(i as u32))
        .collect()
}

/// Faces with at least one vertex inside the sphere.
pub fn faces_in_sphere(mesh: &Mesh, center: Vec3, radius: f32) -> Vec<u32> {
    if mesh.accel.is_some() {
        // Cheaper than a second grid: a face qualifies exactly when one of its
        // vertices does, and adjacency already gives us that.
        let verts = verts_in_sphere(mesh, center, radius);
        let mut seen: FxHashSet<u32> = FxHashSet::default();
        let mut out = Vec::with_capacity(verts.len() * 2);
        for v in verts {
            for &f in &mesh.vfaces[v as usize] {
                if seen.insert(f) {
                    out.push(f);
                }
            }
        }
        return out;
    }
    let r2 = radius * radius;
    mesh.faces
        .par_iter()
        .enumerate()
        .filter_map(|(i, tri)| {
            let hit = tri
                .iter()
                .any(|&v| mesh.verts[v as usize].pos.distance_squared(center) <= r2);
            hit.then_some(i as u32)
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub face: u32,
    pub point: Vec3,
    pub normal: Vec3,
    pub t: f32,
}

const EPS: f32 = 1e-7;

/// Moller-Trumbore against one face.
#[inline]
pub fn ray_face(mesh: &Mesh, fi: u32, origin: Vec3, dir: Vec3) -> Option<Hit> {
    let [a, b, c] = mesh.faces[fi as usize];
    let pa = mesh.verts[a as usize].pos;
    let pb = mesh.verts[b as usize].pos;
    let pc = mesh.verts[c as usize].pos;

    let e1 = pb - pa;
    let e2 = pc - pa;
    let h = dir.cross(e2);
    let det = e1.dot(h);
    if det.abs() < EPS {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - pa;
    let u = s.dot(h) * inv;
    if !(-EPS..=1.0 + EPS).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < -EPS || u + v > 1.0 + EPS {
        return None;
    }
    let t = e2.dot(q) * inv;
    if t <= EPS {
        return None;
    }
    Some(Hit {
        face: fi,
        point: origin + dir * t,
        normal: e1.cross(e2).normalize_or(Vec3::Y),
        t,
    })
}

/// Nearest surface hit along the ray.
pub fn raycast(mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<Hit> {
    if let Some(g) = &mesh.accel {
        if let Some(res) = g.raycast(mesh, origin, dir) {
            return res;
        }
    }
    mesh.faces
        .par_iter()
        .enumerate()
        .filter_map(|(fi, _)| ray_face(mesh, fi as u32, origin, dir))
        .min_by(|x, y| x.t.partial_cmp(&y.t).unwrap_or(std::cmp::Ordering::Equal))
}

/// Nearest surface point to a ray that misses, within `max_dist` of the ray.
///
/// This is what lets a stroke start just off the silhouette. Grabbing the edge
/// of a form means putting the cursor slightly outside it, and a plain ray cast
/// answers "nothing there"; this answers "the surface is right here".
///
/// It walks the ray through the grid rather than scanning the model. It used to
/// scan, on the reasoning that it only runs when the ray missed. That reasoning
/// was wrong: the ray misses on every frame the cursor is not exactly over the
/// model, and the viewport asks once a frame to draw the brush ring. A pass
/// over every vertex, sixty times a second, is what made half a million
/// triangles crawl.
pub fn nearest_to_ray(mesh: &Mesh, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<Hit> {
    if mesh.verts.is_empty() || max_dist <= 0.0 {
        return None;
    }
    let d = dir.normalize_or(-Vec3::Z);

    if let Some(g) = &mesh.accel {
        // Miss the box and there is nothing to find, for the price of six
        // divisions. This is the answer most of the time.
        let (t0, t1) = g.ray_span(origin, d, max_dist)?;
        // Steps of one radius leave no gap: consecutive spheres of that radius
        // centred a radius apart overlap along the whole corridor.
        let step = max_dist.max(g.cell_size() * 0.5);
        let mut best: Option<(f32, f32, u32)> = None;
        let mut t = t0;
        // A hard cap keeps a grazing ray across a huge model from turning into
        // a long walk; past this the fallback below is the cheaper answer.
        let mut budget = 64;
        loop {
            for v in verts_in_sphere(mesh, origin + d * t, max_dist) {
                let to = mesh.verts[v as usize].pos - origin;
                let along = to.dot(d);
                if along <= 0.0 {
                    continue;
                }
                let off = (to - d * along).length_squared();
                let better = best.is_none_or(|(bo, ba, _)| off < bo || (off == bo && along < ba));
                if better {
                    best = Some((off, along, v));
                }
            }
            budget -= 1;
            if t >= t1 || budget == 0 {
                break;
            }
            t = (t + step).min(t1);
        }
        if let Some((_, along, index)) = best {
            return Some(hit_at_vertex(mesh, index, along));
        }
        if budget > 0 {
            // The walk covered the whole corridor and found nothing.
            return None;
        }
    }

    // No grid, or a walk too long to be worth it.
    let limit = max_dist * max_dist;
    let best = mesh
        .verts
        .par_iter()
        .enumerate()
        .filter_map(|(i, v)| {
            let to = v.pos - origin;
            let along = to.dot(d);
            if along <= 0.0 {
                return None; // behind the camera
            }
            let off = (to - d * along).length_squared();
            (off <= limit).then_some((off, along, i as u32))
        })
        // Nearest to the ray line, and among those the nearest to the eye.
        .min_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        })?;
    let (_, along, index) = best;
    Some(hit_at_vertex(mesh, index, along))
}

fn hit_at_vertex(mesh: &Mesh, index: u32, along: f32) -> Hit {
    let v = &mesh.verts[index as usize];
    let face = mesh.vfaces[index as usize].first().copied().unwrap_or(0);
    Hit { face, point: v.pos, normal: v.nrm, t: along }
}

/// Nearest vertex to a point within `radius`, for colour picking and snapping.
pub fn nearest_vertex(mesh: &Mesh, p: Vec3, radius: f32) -> Option<u32> {
    verts_in_sphere(mesh, p, radius)
        .into_iter()
        .map(|v| (v, mesh.verts[v as usize].pos.distance_squared(p)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(v, _)| v)
}
