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

/// Nearest vertex to a point within `radius`, for colour picking and snapping.
pub fn nearest_vertex(mesh: &Mesh, p: Vec3, radius: f32) -> Option<u32> {
    verts_in_sphere(mesh, p, radius)
        .into_iter()
        .map(|v| (v, mesh.verts[v as usize].pos.distance_squared(p)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(v, _)| v)
}
