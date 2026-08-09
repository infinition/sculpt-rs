//! Starting shapes.

use crate::mesh::Mesh;
use glam::Vec3;
use rustc_hash::FxHashMap;
use std::f32::consts::{PI, TAU};

/// Welds vertices that sit within `eps` of each other.
pub fn weld(positions: &[Vec3], faces: &[[u32; 3]], eps: f32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (p, f, _) = weld_indexed(positions, faces, eps);
    (p, f)
}

/// Same as [`weld`], but also hands back `remap[i]`: where source vertex `i`
/// ended up. Callers that carry per-vertex attributes need it.
pub fn weld_indexed(
    positions: &[Vec3],
    faces: &[[u32; 3]],
    eps: f32,
) -> (Vec<Vec3>, Vec<[u32; 3]>, Vec<u32>) {
    let inv = 1.0 / eps;
    let key = |p: Vec3| {
        (
            (p.x * inv).round() as i64,
            (p.y * inv).round() as i64,
            (p.z * inv).round() as i64,
        )
    };
    let mut map: FxHashMap<(i64, i64, i64), u32> = FxHashMap::default();
    let mut out_pos = Vec::new();
    let mut remap = vec![0u32; positions.len()];
    for (i, &p) in positions.iter().enumerate() {
        let k = key(p);
        let id = *map.entry(k).or_insert_with(|| {
            out_pos.push(p);
            (out_pos.len() - 1) as u32
        });
        remap[i] = id;
    }
    let mut out_faces = Vec::with_capacity(faces.len());
    for tri in faces {
        let t = [remap[tri[0] as usize], remap[tri[1] as usize], remap[tri[2] as usize]];
        if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
            out_faces.push(t);
        }
    }
    (out_pos, out_faces, remap)
}

/// Geodesic sphere. Even triangle distribution, which is what you want as a
/// sculpting base: no pole pinching.
pub fn icosphere(subdivisions: u32) -> Mesh {
    let t = (1.0 + 5f32.sqrt()) * 0.5;
    let mut pos: Vec<Vec3> = vec![
        Vec3::new(-1.0, t, 0.0),
        Vec3::new(1.0, t, 0.0),
        Vec3::new(-1.0, -t, 0.0),
        Vec3::new(1.0, -t, 0.0),
        Vec3::new(0.0, -1.0, t),
        Vec3::new(0.0, 1.0, t),
        Vec3::new(0.0, -1.0, -t),
        Vec3::new(0.0, 1.0, -t),
        Vec3::new(t, 0.0, -1.0),
        Vec3::new(t, 0.0, 1.0),
        Vec3::new(-t, 0.0, -1.0),
        Vec3::new(-t, 0.0, 1.0),
    ];
    for p in pos.iter_mut() {
        *p = p.normalize();
    }
    let mut faces: Vec<[u32; 3]> = vec![
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
        [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
        [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];

    for _ in 0..subdivisions {
        let mut cache: FxHashMap<(u32, u32), u32> = FxHashMap::default();
        let mut next = Vec::with_capacity(faces.len() * 4);
        for tri in &faces {
            let mut mid = [0u32; 3];
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                let key = if a < b { (a, b) } else { (b, a) };
                mid[k] = *cache.entry(key).or_insert_with(|| {
                    let m = ((pos[a as usize] + pos[b as usize]) * 0.5).normalize();
                    pos.push(m);
                    (pos.len() - 1) as u32
                });
            }
            next.push([tri[0], mid[0], mid[2]]);
            next.push([tri[1], mid[1], mid[0]]);
            next.push([tri[2], mid[2], mid[1]]);
            next.push([mid[0], mid[1], mid[2]]);
        }
        faces = next;
    }

    Mesh::from_soup(&pos, &faces)
}

/// Latitude/longitude sphere.
pub fn uv_sphere(segments: u32, rings: u32) -> Mesh {
    let mut pos = Vec::new();
    let mut faces = Vec::new();
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let phi = v * PI;
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let theta = u * TAU;
            pos.push(Vec3::new(
                phi.sin() * theta.cos(),
                phi.cos(),
                phi.sin() * theta.sin(),
            ));
        }
    }
    let stride = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * stride + s;
            let b = a + stride;
            faces.push([a, b, a + 1]);
            faces.push([a + 1, b, b + 1]);
        }
    }
    let (p, f) = weld(&pos, &faces, 1e-5);
    Mesh::from_soup(&p, &f)
}

/// Subdivided cube, welded along the seams.
pub fn cube(divisions: u32) -> Mesh {
    let n = divisions.max(1);
    let mut pos = Vec::new();
    let mut faces = Vec::new();
    // (origin, edge u, edge v) for each face, wound outward.
    let axes: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::new(-1.0, -1.0, 1.0), Vec3::X, Vec3::Y),
        (Vec3::new(1.0, -1.0, -1.0), -Vec3::X, Vec3::Y),
        (Vec3::new(-1.0, 1.0, 1.0), Vec3::X, -Vec3::Z),
        (Vec3::new(-1.0, -1.0, -1.0), Vec3::X, Vec3::Z),
        (Vec3::new(1.0, -1.0, 1.0), Vec3::Z * -1.0, Vec3::Y),
        (Vec3::new(-1.0, -1.0, -1.0), Vec3::Z, Vec3::Y),
    ];
    for (origin, du, dv) in axes {
        let base = pos.len() as u32;
        let stride = n + 1;
        for j in 0..=n {
            for i in 0..=n {
                let fu = i as f32 / n as f32 * 2.0;
                let fv = j as f32 / n as f32 * 2.0;
                pos.push(origin + du * fu + dv * fv);
            }
        }
        for j in 0..n {
            for i in 0..n {
                let a = base + j * stride + i;
                let b = a + stride;
                faces.push([a, b, a + 1]);
                faces.push([a + 1, b, b + 1]);
            }
        }
    }
    let (p, f) = weld(&pos, &faces, 1e-4);
    let mut m = Mesh::from_soup(&p, &f);
    m.normalize_scale();
    m
}

/// Capped cylinder.
pub fn cylinder(segments: u32, rings: u32) -> Mesh {
    let mut pos = Vec::new();
    let mut faces = Vec::new();
    let stride = segments + 1;
    for r in 0..=rings {
        let y = -1.0 + 2.0 * (r as f32 / rings as f32);
        for s in 0..=segments {
            let a = s as f32 / segments as f32 * TAU;
            pos.push(Vec3::new(a.cos(), y, a.sin()));
        }
    }
    for r in 0..rings {
        for s in 0..segments {
            let a = r * stride + s;
            let b = a + stride;
            faces.push([a, b, a + 1]);
            faces.push([a + 1, b, b + 1]);
        }
    }
    let bot = pos.len() as u32;
    pos.push(Vec3::new(0.0, -1.0, 0.0));
    let top = pos.len() as u32;
    pos.push(Vec3::new(0.0, 1.0, 0.0));
    for s in 0..segments {
        faces.push([bot, s + 1, s]);
        let base = rings * stride;
        faces.push([top, base + s, base + s + 1]);
    }
    let (p, f) = weld(&pos, &faces, 1e-5);
    Mesh::from_soup(&p, &f)
}

pub fn torus(segments: u32, sides: u32, inner: f32) -> Mesh {
    let mut pos = Vec::new();
    let mut faces = Vec::new();
    for i in 0..=segments {
        let u = i as f32 / segments as f32 * TAU;
        for j in 0..=sides {
            let v = j as f32 / sides as f32 * TAU;
            let r = 1.0 + inner * v.cos();
            pos.push(Vec3::new(r * u.cos(), inner * v.sin(), r * u.sin()));
        }
    }
    let stride = sides + 1;
    for i in 0..segments {
        for j in 0..sides {
            let a = i * stride + j;
            let b = a + stride;
            faces.push([a, b, a + 1]);
            faces.push([a + 1, b, b + 1]);
        }
    }
    let (p, f) = weld(&pos, &faces, 1e-5);
    let mut m = Mesh::from_soup(&p, &f);
    m.normalize_scale();
    m
}

pub fn plane(divisions: u32) -> Mesh {
    let n = divisions.max(1);
    let stride = n + 1;
    let mut pos = Vec::new();
    let mut faces = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let x = -1.0 + 2.0 * i as f32 / n as f32;
            let z = -1.0 + 2.0 * j as f32 / n as f32;
            pos.push(Vec3::new(x, 0.0, z));
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = j * stride + i;
            let b = a + stride;
            faces.push([a, a + 1, b]);
            faces.push([a + 1, b + 1, b]);
        }
    }
    Mesh::from_soup(&pos, &faces)
}
