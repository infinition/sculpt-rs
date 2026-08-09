//! Whole-mesh operations: subdivision, decimation, voxel remeshing, hole
//! filling, mask utilities and extraction.
//!
//! Unlike the brushes, these are one-shot commands. They favour clarity and
//! robustness over speed, and they all return a freshly built mesh or mutate in
//! place through the safe `Mesh` primitives, so the spatial index and adjacency
//! never go stale.

use crate::accel::Grid;
use crate::mesh::{Mesh, Vertex};
use glam::Vec3;
use rayon::prelude::*;
use rustc_hash::FxHashMap;

// ---------------------------------------------------------------------------
// Subdivision
// ---------------------------------------------------------------------------

/// Splits every triangle into four. `smooth` switches from plain midpoint
/// insertion to the Loop scheme, which also relaxes the original vertices.
pub fn subdivide(mesh: &Mesh, smooth: bool) -> Mesh {
    let n0 = mesh.verts.len();
    let mut verts: Vec<Vertex> = mesh.verts.clone();
    let mut faces: Vec<[u32; 3]> = Vec::with_capacity(mesh.faces.len() * 4);
    let mut edge_pt: FxHashMap<(u32, u32), u32> = FxHashMap::default();

    for tri in &mesh.faces {
        let mut mid = [0u32; 3];
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            let key = if a < b { (a, b) } else { (b, a) };
            mid[k] = match edge_pt.get(&key) {
                Some(&id) => id,
                None => {
                    let mut v = Vertex::lerp_attrs(
                        &mesh.verts[a as usize],
                        &mesh.verts[b as usize],
                        0.5,
                    );
                    if smooth {
                        // Loop edge mask: 3/8 on the edge ends, 1/8 on the two
                        // vertices opposite the edge.
                        let opp = opposite_vertices(mesh, a, b);
                        if opp.len() == 2 {
                            let pa = mesh.verts[a as usize].pos;
                            let pb = mesh.verts[b as usize].pos;
                            let pc = mesh.verts[opp[0] as usize].pos;
                            let pd = mesh.verts[opp[1] as usize].pos;
                            v.pos = (pa + pb) * 0.375 + (pc + pd) * 0.125;
                        }
                    }
                    verts.push(v);
                    let id = (verts.len() - 1) as u32;
                    edge_pt.insert(key, id);
                    id
                }
            };
        }
        faces.push([tri[0], mid[0], mid[2]]);
        faces.push([tri[1], mid[1], mid[0]]);
        faces.push([tri[2], mid[2], mid[1]]);
        faces.push([mid[0], mid[1], mid[2]]);
    }

    if smooth {
        // Loop vertex mask on the originals, computed from the source mesh.
        let moved: Vec<Vec3> = (0..n0)
            .into_par_iter()
            .map(|i| {
                let v = i as u32;
                let nb = mesh.neighbors(v);
                let n = nb.len();
                if n < 3 || mesh.is_boundary_vertex(v) {
                    return mesh.verts[i].pos;
                }
                let nf = n as f32;
                let beta = (5.0 / 8.0 - (3.0 / 8.0 + 0.25 * (std::f32::consts::TAU / nf).cos()).powi(2)) / nf;
                let mut sum = Vec3::ZERO;
                for &j in &nb {
                    sum += mesh.verts[j as usize].pos;
                }
                mesh.verts[i].pos * (1.0 - nf * beta) + sum * beta
            })
            .collect();
        for (i, p) in moved.into_iter().enumerate() {
            verts[i].pos = p;
        }
    }

    finish(verts, faces)
}

/// The (up to two) vertices facing an edge across its incident triangles.
fn opposite_vertices(mesh: &Mesh, a: u32, b: u32) -> smallvec::SmallVec<[u32; 2]> {
    let mut out = smallvec::SmallVec::new();
    for f in mesh.faces_around_edge(a, b) {
        for &v in &mesh.faces[f as usize] {
            if v != a && v != b {
                out.push(v);
            }
        }
    }
    out
}

/// Builds a mesh from ready-made vertex and face arrays.
fn finish(verts: Vec<Vertex>, faces: Vec<[u32; 3]>) -> Mesh {
    let mut m = Mesh {
        vfaces: vec![Default::default(); verts.len()],
        verts,
        faces,
        accel: None,
    };
    m.rebuild_adjacency();
    m.recompute_normals();
    m
}

// ---------------------------------------------------------------------------
// Decimation
// ---------------------------------------------------------------------------

/// Collapses short edges until the face count drops to `ratio` of the original.
///
/// Shortest-edge-first is not as sharp as a quadric metric, but it reuses the
/// same guarded collapse the dynamic topology relies on, so the result is
/// always a valid manifold.
pub fn decimate(mesh: &mut Mesh, ratio: f32) -> usize {
    let target = ((mesh.face_count() as f32) * ratio.clamp(0.02, 0.99)) as usize;
    let mut removed = 0;
    mesh.invalidate_accel();

    for _ in 0..40 {
        if mesh.face_count() <= target {
            break;
        }
        let mut edges: Vec<(u32, u32, f32)> = Vec::with_capacity(mesh.faces.len() * 3 / 2);
        {
            let mut seen: rustc_hash::FxHashSet<(u32, u32)> = Default::default();
            for tri in &mesh.faces {
                for k in 0..3 {
                    let (a, b) = (tri[k], tri[(k + 1) % 3]);
                    let key = if a < b { (a, b) } else { (b, a) };
                    if seen.insert(key) {
                        edges.push((key.0, key.1, mesh.edge_len(key.0, key.1)));
                    }
                }
            }
        }
        edges.sort_unstable_by(|x, y| x.2.partial_cmp(&y.2).unwrap_or(std::cmp::Ordering::Equal));

        let before = mesh.face_count();
        for (a, b, _) in edges {
            if mesh.face_count() <= target {
                break;
            }
            let n = mesh.vert_count() as u32;
            if a >= n || b >= n || a == b {
                continue;
            }
            if mesh.faces_around_edge(a, b).is_empty() {
                continue;
            }
            if mesh.collapse_edge(a, b) {
                removed += 2;
            }
        }
        if mesh.face_count() == before {
            break; // no further progress is possible
        }
    }

    mesh.recompute_normals();
    removed
}

// ---------------------------------------------------------------------------
// Voxel remeshing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct RemeshOptions {
    /// Cells along the longest axis.
    pub resolution: u32,
    /// Laplacian passes applied to the result.
    pub smoothing: u32,
    /// Carry vertex colours and materials over from the source.
    pub transfer_colors: bool,
}

impl Default for RemeshOptions {
    fn default() -> Self {
        Self { resolution: 150, smoothing: 2, transfer_colors: true }
    }
}

/// Rebuilds the mesh as a uniform shell using a signed distance field and
/// naive surface nets.
///
/// The sign comes from a scanline parity test rather than a flood fill, so thin
/// walls do not leak, and the field itself is only evaluated in a narrow band
/// around the surface.
pub fn voxel_remesh(mesh: &Mesh, opts: &RemeshOptions) -> Mesh {
    if mesh.faces.is_empty() {
        return Mesh::new();
    }
    let res = opts.resolution.clamp(16, 400);
    let (lo, hi) = mesh.bounds();
    let extent = hi - lo;
    let h = (extent.max_element() / res as f32).max(1e-6);
    let pad = Vec3::splat(h * 2.0);
    let origin = lo - pad;
    let size = extent + pad * 2.0;
    let dims = [
        (size.x / h).ceil() as usize + 2,
        (size.y / h).ceil() as usize + 2,
        (size.z / h).ceil() as usize + 2,
    ];
    let (nx, ny, nz) = (dims[0], dims[1], dims[2]);
    let total = nx * ny * nz;
    if total > 64_000_000 {
        return mesh.clone();
    }

    let band = h * 2.5;
    let mut field = vec![band; total];
    let idx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;

    // Unsigned distance inside a narrow band around every triangle.
    for tri in &mesh.faces {
        let a = mesh.verts[tri[0] as usize].pos;
        let b = mesh.verts[tri[1] as usize].pos;
        let c = mesh.verts[tri[2] as usize].pos;
        let tlo = a.min(b).min(c) - Vec3::splat(band);
        let thi = a.max(b).max(c) + Vec3::splat(band);
        let i0 = (((tlo.x - origin.x) / h).floor().max(0.0) as usize).min(nx - 1);
        let j0 = (((tlo.y - origin.y) / h).floor().max(0.0) as usize).min(ny - 1);
        let k0 = (((tlo.z - origin.z) / h).floor().max(0.0) as usize).min(nz - 1);
        let i1 = (((thi.x - origin.x) / h).ceil().max(0.0) as usize).min(nx - 1);
        let j1 = (((thi.y - origin.y) / h).ceil().max(0.0) as usize).min(ny - 1);
        let k1 = (((thi.z - origin.z) / h).ceil().max(0.0) as usize).min(nz - 1);
        for k in k0..=k1 {
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let p = origin + Vec3::new(i as f32, j as f32, k as f32) * h;
                    let d = point_triangle_distance(p, a, b, c);
                    let slot = &mut field[idx(i, j, k)];
                    if d < *slot {
                        *slot = d;
                    }
                }
            }
        }
    }

    // Sign by parity: rasterise every triangle onto the grid lines running
    // along X and record where it crosses them.
    let mut crossings: Vec<Vec<f32>> = vec![Vec::new(); ny * nz];
    for tri in &mesh.faces {
        let a = mesh.verts[tri[0] as usize].pos;
        let b = mesh.verts[tri[1] as usize].pos;
        let c = mesh.verts[tri[2] as usize].pos;
        let ylo = a.y.min(b.y).min(c.y);
        let yhi = a.y.max(b.y).max(c.y);
        let zlo = a.z.min(b.z).min(c.z);
        let zhi = a.z.max(b.z).max(c.z);
        let j0 = (((ylo - origin.y) / h).ceil().max(0.0) as usize).min(ny);
        let j1 = (((yhi - origin.y) / h).floor().max(0.0) as usize).min(ny - 1);
        let k0 = (((zlo - origin.z) / h).ceil().max(0.0) as usize).min(nz);
        let k1 = (((zhi - origin.z) / h).floor().max(0.0) as usize).min(nz - 1);
        if j0 > j1 || k0 > k1 {
            continue;
        }
        // Barycentric setup in the YZ plane.
        let d = (b.y - a.y) * (c.z - a.z) - (c.y - a.y) * (b.z - a.z);
        if d.abs() < 1e-20 {
            continue;
        }
        let inv_d = 1.0 / d;
        for k in k0..=k1 {
            let pz = origin.z + k as f32 * h;
            for j in j0..=j1 {
                let py = origin.y + j as f32 * h;
                let u = ((py - a.y) * (c.z - a.z) - (c.y - a.y) * (pz - a.z)) * inv_d;
                let v = ((b.y - a.y) * (pz - a.z) - (py - a.y) * (b.z - a.z)) * inv_d;
                if u < 0.0 || v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let x = a.x + u * (b.x - a.x) + v * (c.x - a.x);
                crossings[k * ny + j].push(x);
            }
        }
    }

    for (line, xs) in crossings.iter_mut().enumerate() {
        if xs.is_empty() {
            continue;
        }
        xs.sort_unstable_by(|p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
        let k = line / ny;
        let j = line % ny;
        let mut cursor = 0usize;
        let mut inside = false;
        for i in 0..nx {
            let x = origin.x + i as f32 * h;
            while cursor < xs.len() && xs[cursor] <= x {
                inside = !inside;
                cursor += 1;
            }
            if inside {
                let slot = &mut field[idx(i, j, k)];
                *slot = -*slot;
            }
        }
    }

    // Surface nets: one vertex per cell that straddles the surface.
    let cell_dims = (nx - 1, ny - 1, nz - 1);
    let mut cell_vert = vec![u32::MAX; cell_dims.0 * cell_dims.1 * cell_dims.2];
    let cidx = |i: usize, j: usize, k: usize| (k * cell_dims.1 + j) * cell_dims.0 + i;
    let mut positions: Vec<Vec3> = Vec::new();

    const CORNER: [[usize; 3]; 8] = [
        [0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0],
        [0, 0, 1], [1, 0, 1], [0, 1, 1], [1, 1, 1],
    ];
    const EDGE: [[usize; 2]; 12] = [
        [0, 1], [2, 3], [4, 5], [6, 7],
        [0, 2], [1, 3], [4, 6], [5, 7],
        [0, 4], [1, 5], [2, 6], [3, 7],
    ];

    for k in 0..cell_dims.2 {
        for j in 0..cell_dims.1 {
            for i in 0..cell_dims.0 {
                let mut val = [0f32; 8];
                let mut neg = 0;
                for (c, off) in CORNER.iter().enumerate() {
                    let v = field[idx(i + off[0], j + off[1], k + off[2])];
                    val[c] = v;
                    if v < 0.0 {
                        neg += 1;
                    }
                }
                if neg == 0 || neg == 8 {
                    continue;
                }
                let mut acc = Vec3::ZERO;
                let mut count = 0.0;
                for e in EDGE {
                    let (va, vb) = (val[e[0]], val[e[1]]);
                    if (va < 0.0) == (vb < 0.0) {
                        continue;
                    }
                    let t = (va / (va - vb)).clamp(0.0, 1.0);
                    let ca = Vec3::new(
                        CORNER[e[0]][0] as f32,
                        CORNER[e[0]][1] as f32,
                        CORNER[e[0]][2] as f32,
                    );
                    let cb = Vec3::new(
                        CORNER[e[1]][0] as f32,
                        CORNER[e[1]][1] as f32,
                        CORNER[e[1]][2] as f32,
                    );
                    acc += ca.lerp(cb, t);
                    count += 1.0;
                }
                if count == 0.0 {
                    continue;
                }
                let local = acc / count;
                let p = origin + (Vec3::new(i as f32, j as f32, k as f32) + local) * h;
                cell_vert[cidx(i, j, k)] = positions.len() as u32;
                positions.push(p);
            }
        }
    }

    // Quads around every sign-changing grid edge, wound so normals point out.
    let mut faces: Vec<[u32; 3]> = Vec::new();
    let quad = |a: u32, b: u32, c: u32, d: u32, flip: bool, faces: &mut Vec<[u32; 3]>| {
        if a == u32::MAX || b == u32::MAX || c == u32::MAX || d == u32::MAX {
            return;
        }
        if flip {
            faces.push([a, b, c]);
            faces.push([a, c, d]);
        } else {
            faces.push([a, c, b]);
            faces.push([a, d, c]);
        }
    };

    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let here = field[idx(i, j, k)] < 0.0;
                // +X edge, shared by the four cells around it in Y and Z.
                if i + 1 < nx && j > 0 && k > 0 && j < ny && k < nz {
                    let there = field[idx(i + 1, j, k)] < 0.0;
                    if here != there {
                        let a = cell_vert[cidx(i, j - 1, k - 1)];
                        let b = cell_vert[cidx(i, j, k - 1)];
                        let c = cell_vert[cidx(i, j, k)];
                        let d = cell_vert[cidx(i, j - 1, k)];
                        quad(a, b, c, d, here, &mut faces);
                    }
                }
                if j + 1 < ny && i > 0 && k > 0 && i < nx && k < nz {
                    let there = field[idx(i, j + 1, k)] < 0.0;
                    if here != there {
                        let a = cell_vert[cidx(i - 1, j, k - 1)];
                        let b = cell_vert[cidx(i, j, k - 1)];
                        let c = cell_vert[cidx(i, j, k)];
                        let d = cell_vert[cidx(i - 1, j, k)];
                        quad(a, b, c, d, !here, &mut faces);
                    }
                }
                if k + 1 < nz && i > 0 && j > 0 && i < nx && j < ny {
                    let there = field[idx(i, j, k + 1)] < 0.0;
                    if here != there {
                        let a = cell_vert[cidx(i - 1, j - 1, k)];
                        let b = cell_vert[cidx(i, j - 1, k)];
                        let c = cell_vert[cidx(i, j, k)];
                        let d = cell_vert[cidx(i - 1, j, k)];
                        quad(a, b, c, d, here, &mut faces);
                    }
                }
            }
        }
    }

    if positions.is_empty() || faces.is_empty() {
        return mesh.clone();
    }

    let mut verts: Vec<Vertex> = positions.iter().map(|p| Vertex::new(*p)).collect();
    if opts.transfer_colors {
        let grid = Grid::build(mesh, crate::accel::ideal_cell(h * 3.0));
        let samples: Vec<(Vec3, f32, f32, f32)> = verts
            .par_iter()
            .map(|v| {
                let near = grid
                    .verts_in_sphere(mesh, v.pos, h * 3.0)
                    .unwrap_or_default();
                let best = near
                    .into_iter()
                    .map(|i| (i, mesh.verts[i as usize].pos.distance_squared(v.pos)))
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                match best {
                    Some((i, _)) => {
                        let s = &mesh.verts[i as usize];
                        (s.col, s.mask, s.rough, s.metal)
                    }
                    None => (Vec3::splat(0.85), 0.0, 0.6, 0.0),
                }
            })
            .collect();
        for (v, s) in verts.iter_mut().zip(samples) {
            v.col = s.0;
            v.mask = s.1;
            v.rough = s.2;
            v.metal = s.3;
        }
    }

    let mut out = finish(verts, faces);
    for _ in 0..opts.smoothing {
        laplacian_smooth(&mut out, 0.5);
    }
    out.recompute_normals();
    out
}

/// One global Laplacian pass, keeping boundary vertices pinned.
pub fn laplacian_smooth(mesh: &mut Mesh, amount: f32) {
    let deltas: Vec<Vec3> = (0..mesh.verts.len())
        .into_par_iter()
        .map(|i| {
            let v = i as u32;
            let nb = mesh.neighbors(v);
            if nb.is_empty() || mesh.is_boundary_vertex(v) {
                return Vec3::ZERO;
            }
            let mut mean = Vec3::ZERO;
            for &n in &nb {
                mean += mesh.verts[n as usize].pos;
            }
            mean /= nb.len() as f32;
            (mean - mesh.verts[i].pos) * amount
        })
        .collect();
    for (v, d) in mesh.verts.iter_mut().zip(deltas) {
        v.pos += d;
    }
    mesh.invalidate_accel();
}

/// Squared-free distance from a point to a triangle.
fn point_triangle_distance(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length();
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (ap - ab * v).length();
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (ap - ac * w).length();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (p - (b + (c - b) * w)).length();
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (p - (a + ab * v + ac * w)).length()
}

// ---------------------------------------------------------------------------
// Hole filling
// ---------------------------------------------------------------------------

/// Fans every boundary loop to its centroid. Returns the number of holes filled.
pub fn close_holes(mesh: &mut Mesh) -> usize {
    let loops = mesh.boundary_loops();
    if loops.is_empty() {
        return 0;
    }
    for chain in &loops {
        let mut center = Vec3::ZERO;
        let mut col = Vec3::ZERO;
        for &v in chain {
            center += mesh.verts[v as usize].pos;
            col += mesh.verts[v as usize].col;
        }
        let n = chain.len() as f32;
        let mut cv = Vertex::new(center / n);
        cv.col = col / n;
        let c = mesh.add_vertex(cv);
        for i in 0..chain.len() {
            let a = chain[i];
            let b = chain[(i + 1) % chain.len()];
            // The boundary edge runs a -> b inside its own face, so the patch
            // has to run the other way to keep the winding outward.
            mesh.add_face([b, a, c]);
        }
    }
    mesh.recompute_normals();
    mesh.invalidate_accel();
    loops.len()
}

// ---------------------------------------------------------------------------
// Mask utilities
// ---------------------------------------------------------------------------

pub fn clear_mask(mesh: &mut Mesh) {
    mesh.verts.par_iter_mut().for_each(|v| v.mask = 0.0);
}

pub fn invert_mask(mesh: &mut Mesh) {
    mesh.verts.par_iter_mut().for_each(|v| v.mask = 1.0 - v.mask);
}

/// Blurs (`amount > 0`) or sharpens (`amount < 0`) the mask.
pub fn filter_mask(mesh: &mut Mesh, amount: f32) {
    let vals: Vec<f32> = (0..mesh.verts.len())
        .into_par_iter()
        .map(|i| {
            let nb = mesh.neighbors(i as u32);
            if nb.is_empty() {
                return mesh.verts[i].mask;
            }
            let mut mean = 0.0;
            for &n in &nb {
                mean += mesh.verts[n as usize].mask;
            }
            mean /= nb.len() as f32;
            let m = mesh.verts[i].mask;
            (m + (mean - m) * amount).clamp(0.0, 1.0)
        })
        .collect();
    for (v, m) in mesh.verts.iter_mut().zip(vals) {
        v.mask = m;
    }
}

/// Lifts the masked region into a separate closed shell of `thickness`.
///
/// This is the counterpart of SculptGL's extract: the masked patch is copied,
/// offset inward along its normals, and the two shells are stitched along the
/// patch border.
pub fn extract_masked(mesh: &Mesh, thickness: f32) -> Option<Mesh> {
    let keep: Vec<usize> = mesh
        .faces
        .iter()
        .enumerate()
        .filter(|(_, tri)| {
            tri.iter().all(|&v| mesh.verts[v as usize].mask > 0.5)
        })
        .map(|(i, _)| i)
        .collect();
    if keep.is_empty() {
        return None;
    }

    // Remap the used vertices into a dense patch.
    let mut remap: FxHashMap<u32, u32> = FxHashMap::default();
    let mut verts: Vec<Vertex> = Vec::new();
    let mut faces: Vec<[u32; 3]> = Vec::with_capacity(keep.len());
    for &f in &keep {
        let tri = mesh.faces[f];
        let mut out = [0u32; 3];
        for k in 0..3 {
            let src = tri[k];
            out[k] = *remap.entry(src).or_insert_with(|| {
                let mut v = mesh.verts[src as usize];
                v.mask = 0.0;
                verts.push(v);
                (verts.len() - 1) as u32
            });
        }
        faces.push(out);
    }

    let patch = finish(verts, faces);
    let n = patch.verts.len() as u32;

    // Outer shell, then the inner shell offset along the normals and flipped.
    let mut verts = patch.verts.clone();
    let mut faces = patch.faces.clone();
    for i in 0..n {
        let mut v = patch.verts[i as usize];
        v.pos -= v.nrm * thickness;
        verts.push(v);
    }
    for tri in &patch.faces {
        faces.push([tri[2] + n, tri[1] + n, tri[0] + n]);
    }
    for chain in patch.boundary_loops() {
        for i in 0..chain.len() {
            let a = chain[i];
            let b = chain[(i + 1) % chain.len()];
            faces.push([b, a, a + n]);
            faces.push([b, a + n, b + n]);
        }
    }

    Some(finish(verts, faces))
}

// ---------------------------------------------------------------------------
// Colour fill
// ---------------------------------------------------------------------------

/// Floods colour outward from one face.
///
/// `Region` walks the face graph and refuses to cross an edge whose dihedral
/// angle is sharper than `angle_deg`, which is what makes a flat panel fill as
/// one piece while the bevel beside it stays untouched. Masked vertices are
/// left alone, so a mask doubles as a fill boundary.
pub fn fill(
    mesh: &mut Mesh,
    start_face: u32,
    scope: crate::brush::FillScope,
    color: Vec3,
    blend: crate::brush::BlendMode,
    amount: f32,
    angle_deg: f32,
) -> usize {
    use crate::brush::FillScope;

    let faces: Vec<u32> = match scope {
        FillScope::Object => (0..mesh.face_count() as u32).collect(),
        FillScope::Face => vec![start_face],
        FillScope::Region => {
            if start_face as usize >= mesh.face_count() {
                return 0;
            }
            let limit = angle_deg.to_radians().cos();
            let mut seen = vec![false; mesh.face_count()];
            let mut stack = vec![start_face];
            let mut out = Vec::new();
            seen[start_face as usize] = true;
            while let Some(f) = stack.pop() {
                out.push(f);
                let n0 = mesh.face_normal(f).normalize_or(Vec3::Y);
                let tri = mesh.faces[f as usize];
                for k in 0..3 {
                    let (a, b) = (tri[k], tri[(k + 1) % 3]);
                    for nb in mesh.faces_around_edge(a, b) {
                        if nb == f || seen[nb as usize] {
                            continue;
                        }
                        let n1 = mesh.face_normal(nb).normalize_or(Vec3::Y);
                        if n0.dot(n1) < limit {
                            continue; // too sharp a crease to spill over
                        }
                        seen[nb as usize] = true;
                        stack.push(nb);
                    }
                }
            }
            out
        }
    };

    let mut touched: Vec<u32> = Vec::with_capacity(faces.len() * 3);
    for f in faces {
        touched.extend_from_slice(&mesh.faces[f as usize]);
    }
    touched.sort_unstable();
    touched.dedup();

    for &v in &touched {
        let vx = &mut mesh.verts[v as usize];
        let t = (amount * (1.0 - vx.mask)).clamp(0.0, 1.0);
        let blended = blend.apply(vx.col, color);
        vx.col = vx.col.lerp(blended, t);
    }
    touched.len()
}

// ---------------------------------------------------------------------------
// Symmetry
// ---------------------------------------------------------------------------

/// Mirrors the whole mesh across a plane through the origin.
pub fn mirror(mesh: &mut Mesh, axis: crate::brush::Axis) {
    let i = axis.index();
    mesh.verts.par_iter_mut().for_each(|v| {
        v.pos[i] = -v.pos[i];
        v.nrm[i] = -v.nrm[i];
    });
    mesh.faces.par_iter_mut().for_each(|t| t.swap(1, 2));
    mesh.rebuild_adjacency();
    mesh.recompute_normals();
}

/// Keeps one half of the mesh and replaces the other with its mirror image,
/// clipping the triangles that straddle the plane.
pub fn symmetrize(mesh: &Mesh, axis: crate::brush::Axis, keep_positive: bool) -> Mesh {
    let i = axis.index();
    let side = |p: Vec3| if keep_positive { p[i] } else { -p[i] };

    let mut verts: Vec<Vertex> = Vec::with_capacity(mesh.verts.len());
    let mut faces: Vec<[u32; 3]> = Vec::with_capacity(mesh.faces.len());
    let push = |v: Vertex, verts: &mut Vec<Vertex>| {
        verts.push(v);
        (verts.len() - 1) as u32
    };

    for tri in &mesh.faces {
        let p: [Vertex; 3] = [
            mesh.verts[tri[0] as usize],
            mesh.verts[tri[1] as usize],
            mesh.verts[tri[2] as usize],
        ];
        let d = [side(p[0].pos), side(p[1].pos), side(p[2].pos)];
        let inside = d.iter().filter(|x| **x >= 0.0).count();
        if inside == 0 {
            continue;
        }
        if inside == 3 {
            let a = push(p[0], &mut verts);
            let b = push(p[1], &mut verts);
            let c = push(p[2], &mut verts);
            faces.push([a, b, c]);
            continue;
        }
        // Clip the triangle against the plane, keeping the positive part.
        let mut poly: Vec<Vertex> = Vec::with_capacity(4);
        for k in 0..3 {
            let (cur, nxt) = (k, (k + 1) % 3);
            if d[cur] >= 0.0 {
                poly.push(p[cur]);
            }
            if (d[cur] >= 0.0) != (d[nxt] >= 0.0) {
                let t = d[cur] / (d[cur] - d[nxt]);
                let mut v = Vertex::lerp_attrs(&p[cur], &p[nxt], t);
                v.pos[i] = 0.0;
                poly.push(v);
            }
        }
        for k in 1..poly.len().saturating_sub(1) {
            let a = push(poly[0], &mut verts);
            let b = push(poly[k], &mut verts);
            let c = push(poly[k + 1], &mut verts);
            faces.push([a, b, c]);
        }
    }

    if faces.is_empty() {
        return mesh.clone();
    }

    // Mirror the kept half and append it with reversed winding.
    let n = verts.len() as u32;
    let mirrored: Vec<Vertex> = verts
        .iter()
        .map(|v| {
            let mut m = *v;
            m.pos[i] = -m.pos[i];
            m.nrm[i] = -m.nrm[i];
            m
        })
        .collect();
    let extra: Vec<[u32; 3]> = faces.iter().map(|t| [t[2] + n, t[1] + n, t[0] + n]).collect();
    verts.extend(mirrored);
    faces.extend(extra);

    // The two halves meet on the plane with coincident vertices; weld them.
    let positions: Vec<Vec3> = verts.iter().map(|v| v.pos).collect();
    let (welded_pos, welded_faces, remap) =
        crate::primitives::weld_indexed(&positions, &faces, 1e-6);
    let mut out_verts: Vec<Vertex> = welded_pos.iter().map(|p| Vertex::new(*p)).collect();
    let mut filled = vec![false; out_verts.len()];
    for (src, &w) in remap.iter().enumerate() {
        if !filled[w as usize] {
            out_verts[w as usize] = verts[src];
            filled[w as usize] = true;
        }
    }
    finish(out_verts, welded_faces)
}
