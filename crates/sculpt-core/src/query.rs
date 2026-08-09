//! Spatial queries.
//!
//! Deliberately brute force over `rayon` rather than an octree. Dynamic
//! topology rewrites the vertex and face arrays on every stroke step, so an
//! incremental acceleration structure costs more in invalidation bookkeeping
//! than it saves below a few million vertices. Swap in a BVH here when that
//! stops being true; nothing else in the crate needs to change.

use crate::mesh::Mesh;
use glam::Vec3;
use rayon::prelude::*;

/// Indices of every vertex inside the sphere.
pub fn verts_in_sphere(mesh: &Mesh, center: Vec3, radius: f32) -> Vec<u32> {
    let r2 = radius * radius;
    mesh.verts
        .par_iter()
        .enumerate()
        .filter_map(|(i, v)| {
            if v.pos.distance_squared(center) <= r2 {
                Some(i as u32)
            } else {
                None
            }
        })
        .collect()
}

/// Faces with at least one vertex inside the sphere.
pub fn faces_in_sphere(mesh: &Mesh, center: Vec3, radius: f32) -> Vec<u32> {
    let r2 = radius * radius;
    mesh.faces
        .par_iter()
        .enumerate()
        .filter_map(|(i, tri)| {
            let hit = tri
                .iter()
                .any(|&v| mesh.verts[v as usize].pos.distance_squared(center) <= r2);
            if hit {
                Some(i as u32)
            } else {
                None
            }
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

/// Moller-Trumbore against every face, keeping the nearest hit.
pub fn raycast(mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<Hit> {
    const EPS: f32 = 1e-7;

    let best = mesh
        .faces
        .par_iter()
        .enumerate()
        .filter_map(|(fi, &[a, b, c])| {
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
            Some((t, fi as u32, e1.cross(e2).normalize_or(Vec3::Y)))
        })
        .min_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));

    best.map(|(t, face, normal)| Hit { face, point: origin + dir * t, normal, t })
}
