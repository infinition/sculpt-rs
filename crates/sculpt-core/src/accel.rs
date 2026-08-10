//! Spatial hash grid over vertices and faces.
//!
//! Sculpting is a local operation: every dab touches a sphere that is tiny
//! compared with the model, yet a brute-force scan pays for the whole mesh on
//! each of the dozens of queries a stroke performs. The grid turns that into a
//! walk over a fixed number of cells.
//!
//! The table is a fixed-size array of buckets addressed by a hash of the cell
//! coordinate, so the grid covers unbounded space and never rehashes; two cells
//! landing in the same bucket only cost a few extra distance tests. Cell size is
//! tied to the brush radius (see [`Grid::wants_rebuild`]), which keeps the
//! number of visited cells constant whatever the zoom level.
//!
//! Everything is maintained incrementally by [`crate::mesh::Mesh`]. A mutation
//! that cannot be tracked cheaply drops the grid instead, and the next query
//! rebuilds it.

use crate::mesh::Mesh;
use rayon::prelude::*;
use crate::query::Hit;
use glam::Vec3;
use smallvec::SmallVec;

/// A cell holds four elements before it reaches for the heap.
///
/// Eight was too generous. A cell is sized to hold a couple of vertices, and
/// every unused slot is paid for across the whole table: at forty bytes a cell
/// over two million cells, building a grid meant writing 168 MB before looking
/// at a single vertex.
type Bucket = SmallVec<[u32; 4]>;

/// Query radius over cell size. 2.0 means a sphere query walks at most 5^3
/// cells, whatever the absolute scale.
const CELLS_PER_RADIUS: f32 = 2.0;
/// Rebuild once the cell size is this far off the ideal, so a slow radius drag
/// does not rebuild every frame.
const REBUILD_SLACK: f32 = 2.5;
/// Guard rails on the DDA in [`Grid::raycast`].
const MAX_RAY_STEPS: usize = 8192;

#[derive(Clone)]
pub struct Grid {
    cell: f32,
    inv: f32,
    mask: u32,
    vbuckets: Vec<Bucket>,
    fbuckets: Vec<Bucket>,
    /// Bucket currently holding each vertex, so removal knows where to look.
    vslot: Vec<u32>,
    fslot: Vec<u32>,
    /// Largest centroid-to-corner distance of any face inserted so far. Faces
    /// are filed by centroid, so a ray has to look this far to the side.
    face_radius: f32,
    lo: Vec3,
    hi: Vec3,
}

#[inline]
fn hash_cell(x: i32, y: i32, z: i32, mask: u32) -> u32 {
    let h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h & mask
}

impl Grid {
    /// Fills a grid sized for `mesh` with cells of `cell` world units.
    pub fn build(mesh: &Mesh, cell: f32) -> Self {
        let n = mesh.verts.len().max(mesh.faces.len()).max(64);
        // Roughly four elements a cell, and a firm ceiling.
        //
        // One element a cell sounds better and is not: the table is written
        // once per build and read a handful of cells at a time, so a table that
        // does not fit in cache costs more in misses than the extra comparisons
        // cost in a cell. This one stays under 6 MB at any mesh size.
        let table = (n / 4).next_power_of_two().clamp(1024, 1 << 17);
        let mut g = Self {
            cell: cell.max(1e-6),
            inv: 1.0 / cell.max(1e-6),
            mask: (table - 1) as u32,
            vbuckets: vec![Bucket::new(); table],
            fbuckets: vec![Bucket::new(); table],
            vslot: vec![0; mesh.verts.len()],
            fslot: vec![0; mesh.faces.len()],
            face_radius: 0.0,
            lo: Vec3::splat(f32::MAX),
            hi: Vec3::splat(f32::MIN),
        };
        // Cells first, in parallel, then the filing. Hashing is arithmetic
        // and splits perfectly across threads; the filing is a scatter that
        // does not.
        let vcells: Vec<u32> = mesh.verts.par_iter().map(|v| g.slot(v.pos)).collect();
        let (lo, hi) = mesh.bounds();
        g.lo = lo;
        g.hi = hi;
        for (i, s) in vcells.iter().enumerate() {
            g.vbuckets[*s as usize].push(i as u32);
            g.vslot[i] = *s;
        }
        let spheres: Vec<(u32, f32)> = (0..mesh.faces.len())
            .into_par_iter()
            .map(|i| {
                let (c, r) = face_sphere(mesh, i as u32);
                (g.slot(c), r)
            })
            .collect();
        for (i, (s, r)) in spheres.into_iter().enumerate() {
            g.fbuckets[s as usize].push(i as u32);
            g.fslot[i] = s;
            g.face_radius = g.face_radius.max(r);
        }
        if mesh.verts.is_empty() {
            g.lo = Vec3::ZERO;
            g.hi = Vec3::ZERO;
        }
        g
    }

    /// The corner of the box every inserted vertex has fallen inside.
    ///
    /// Only ever grown, so it stays a valid bound after a collapse even though
    /// it may end up larger than it needs to be.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        (self.lo, self.hi)
    }

    /// Where a ray enters and leaves the box, widened by `margin`.
    ///
    /// Answers `None` when the ray misses entirely, which is the answer worth
    /// having: it costs nothing and it is the common case for a cursor that is
    /// not over the model.
    pub fn ray_span(&self, o: Vec3, d: Vec3, margin: f32) -> Option<(f32, f32)> {
        let m = Vec3::splat(margin);
        let (t0, t1) = slab(o, d, self.lo - m, self.hi + m)?;
        let t0 = t0.max(0.0);
        (t1 >= t0).then_some((t0, t1))
    }

    pub fn cell_size(&self) -> f32 {
        self.cell
    }

    /// True when `radius` no longer matches the cell size closely enough.
    pub fn wants_rebuild(&self, radius: f32) -> bool {
        let ideal = ideal_cell(radius);
        let ratio = self.cell / ideal;
        !(1.0 / REBUILD_SLACK..=REBUILD_SLACK).contains(&ratio)
    }

    #[inline]
    fn coord(&self, p: Vec3) -> (i32, i32, i32) {
        (
            (p.x * self.inv).floor() as i32,
            (p.y * self.inv).floor() as i32,
            (p.z * self.inv).floor() as i32,
        )
    }

    #[inline]
    fn slot(&self, p: Vec3) -> u32 {
        let (x, y, z) = self.coord(p);
        hash_cell(x, y, z, self.mask)
    }

    #[inline]
    fn grow(&mut self, p: Vec3) {
        self.lo = self.lo.min(p);
        self.hi = self.hi.max(p);
    }

    // ---- incremental maintenance -------------------------------------------

    pub fn push_vertex(&mut self, v: u32, p: Vec3) {
        debug_assert_eq!(self.vslot.len(), v as usize);
        let s = self.slot(p);
        self.vbuckets[s as usize].push(v);
        self.vslot.push(s);
        self.grow(p);
    }

    /// Returns true when the vertex actually changed bucket, which is the only
    /// case where its incident faces need refiling too.
    pub fn move_vertex(&mut self, v: u32, p: Vec3) -> bool {
        self.grow(p);
        let s = self.slot(p);
        let old = self.vslot[v as usize];
        if s == old {
            return false;
        }
        remove_from(&mut self.vbuckets[old as usize], v);
        self.vbuckets[s as usize].push(v);
        self.vslot[v as usize] = s;
        true
    }

    /// Mirrors `Vec::swap_remove`: `v` goes away and the last vertex takes its
    /// index.
    pub fn swap_remove_vertex(&mut self, v: u32) {
        let last = (self.vslot.len() - 1) as u32;
        let s = self.vslot[v as usize];
        remove_from(&mut self.vbuckets[s as usize], v);
        if v != last {
            let ls = self.vslot[last as usize];
            for e in self.vbuckets[ls as usize].iter_mut() {
                if *e == last {
                    *e = v;
                }
            }
            self.vslot[v as usize] = ls;
        }
        self.vslot.pop();
    }

    pub fn push_face(&mut self, f: u32, centroid: Vec3, radius: f32) {
        debug_assert_eq!(self.fslot.len(), f as usize);
        let s = self.slot(centroid);
        self.fbuckets[s as usize].push(f);
        self.fslot.push(s);
        self.face_radius = self.face_radius.max(radius);
    }

    pub fn move_face(&mut self, f: u32, centroid: Vec3, radius: f32) {
        let s = self.slot(centroid);
        let old = self.fslot[f as usize];
        if s != old {
            remove_from(&mut self.fbuckets[old as usize], f);
            self.fbuckets[s as usize].push(f);
            self.fslot[f as usize] = s;
        }
        self.face_radius = self.face_radius.max(radius);
    }

    pub fn swap_remove_face(&mut self, f: u32) {
        let last = (self.fslot.len() - 1) as u32;
        let s = self.fslot[f as usize];
        remove_from(&mut self.fbuckets[s as usize], f);
        if f != last {
            let ls = self.fslot[last as usize];
            for e in self.fbuckets[ls as usize].iter_mut() {
                if *e == last {
                    *e = f;
                }
            }
            self.fslot[f as usize] = ls;
        }
        self.fslot.pop();
    }

    // ---- queries -----------------------------------------------------------

    /// Buckets overlapping the axis-aligned box, deduplicated.
    fn buckets_in_box(&self, lo: Vec3, hi: Vec3, out: &mut Vec<u32>) {
        out.clear();
        let (x0, y0, z0) = self.coord(lo);
        let (x1, y1, z1) = self.coord(hi);
        // A pathological radius/cell ratio would enumerate the world; the caller
        // falls back to a linear scan when this trips.
        let span = (x1 - x0 + 1) as i64 * (y1 - y0 + 1) as i64 * (z1 - z0 + 1) as i64;
        if span > 32_768 {
            return;
        }
        for z in z0..=z1 {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    out.push(hash_cell(x, y, z, self.mask));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }

    /// Indices of every vertex inside the sphere, or `None` when the caller
    /// should fall back to a linear scan.
    pub fn verts_in_sphere(&self, mesh: &Mesh, c: Vec3, r: f32) -> Option<Vec<u32>> {
        let mut slots = Vec::new();
        self.buckets_in_box(c - Vec3::splat(r), c + Vec3::splat(r), &mut slots);
        if slots.is_empty() {
            return None;
        }
        let r2 = r * r;
        let mut out = Vec::new();
        for s in slots {
            for &v in &self.vbuckets[s as usize] {
                if mesh.verts[v as usize].pos.distance_squared(c) <= r2 {
                    out.push(v);
                }
            }
        }
        Some(out)
    }

    /// Nearest ray hit, walking cells front to back. `None` means the caller
    /// should fall back to the linear cast.
    pub fn raycast(&self, mesh: &Mesh, o: Vec3, d: Vec3) -> Option<Option<Hit>> {
        if mesh.faces.is_empty() {
            return Some(None);
        }
        // A face is filed by its centroid, so widen the corridor by the largest
        // centroid-to-corner distance seen. Too wide and the walk is pointless.
        let pad = (self.face_radius * self.inv).ceil().max(0.0) as i32;
        if pad > 3 {
            return None;
        }
        let margin = Vec3::splat(self.face_radius + self.cell);
        let (mut t0, t1) = slab(o, d, self.lo - margin, self.hi + margin)?;
        t0 = t0.max(0.0);
        if t1 < t0 {
            return Some(None);
        }

        let start = o + d * t0;
        let (mut cx, mut cy, mut cz) = self.coord(start);
        let step = [
            if d.x > 0.0 { 1 } else { -1 },
            if d.y > 0.0 { 1 } else { -1 },
            if d.z > 0.0 { 1 } else { -1 },
        ];
        let inv_d = Vec3::new(
            if d.x.abs() > 1e-12 { 1.0 / d.x } else { f32::MAX },
            if d.y.abs() > 1e-12 { 1.0 / d.y } else { f32::MAX },
            if d.z.abs() > 1e-12 { 1.0 / d.z } else { f32::MAX },
        );
        // Distance to the next cell boundary on each axis.
        let mut next = Vec3::new(
            boundary_t(start.x, d.x, cx, self.cell, inv_d.x, t0),
            boundary_t(start.y, d.y, cy, self.cell, inv_d.y, t0),
            boundary_t(start.z, d.z, cz, self.cell, inv_d.z, t0),
        );
        let delta = Vec3::new(
            (self.cell * inv_d.x).abs(),
            (self.cell * inv_d.y).abs(),
            (self.cell * inv_d.z).abs(),
        );

        let mut best: Option<Hit> = None;
        let mut t = t0;
        let slack = self.face_radius + self.cell * 1.75;

        for _ in 0..MAX_RAY_STEPS {
            for z in (cz - pad)..=(cz + pad) {
                for y in (cy - pad)..=(cy + pad) {
                    for x in (cx - pad)..=(cx + pad) {
                        let s = hash_cell(x, y, z, self.mask);
                        for &f in &self.fbuckets[s as usize] {
                            if let Some(h) = crate::query::ray_face(mesh, f, o, d) {
                                if best.as_ref().is_none_or(|b| h.t < b.t) {
                                    best = Some(h);
                                }
                            }
                        }
                    }
                }
            }
            // Everything left to visit is further away than the current hit.
            if let Some(b) = &best {
                if b.t + slack < t {
                    break;
                }
            }
            t = next.min_element();
            if t > t1 {
                break;
            }
            if next.x <= next.y && next.x <= next.z {
                cx += step[0];
                next.x += delta.x;
            } else if next.y <= next.z {
                cy += step[1];
                next.y += delta.y;
            } else {
                cz += step[2];
                next.z += delta.z;
            }
        }
        Some(best)
    }
}

/// Cell size that keeps a sphere query to a handful of cells.
pub fn ideal_cell(radius: f32) -> f32 {
    (radius / CELLS_PER_RADIUS).max(1e-5)
}

#[inline]
fn remove_from(b: &mut Bucket, x: u32) {
    if let Some(i) = b.iter().position(|&e| e == x) {
        b.swap_remove(i);
    }
}

#[inline]
fn face_sphere(mesh: &Mesh, f: u32) -> (Vec3, f32) {
    let [a, b, c] = mesh.faces[f as usize];
    let pa = mesh.verts[a as usize].pos;
    let pb = mesh.verts[b as usize].pos;
    let pc = mesh.verts[c as usize].pos;
    let mid = (pa + pb + pc) / 3.0;
    let r = pa
        .distance(mid)
        .max(pb.distance(mid))
        .max(pc.distance(mid));
    (mid, r)
}

/// Ray/box overlap interval, or `None` when the ray misses.
fn slab(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3) -> Option<(f32, f32)> {
    let mut t0 = f32::NEG_INFINITY;
    let mut t1 = f32::INFINITY;
    for k in 0..3 {
        let (oi, di, li, hi_) = (o[k], d[k], lo[k], hi[k]);
        if di.abs() < 1e-12 {
            if oi < li || oi > hi_ {
                return None;
            }
            continue;
        }
        let inv = 1.0 / di;
        let (mut a, mut b) = ((li - oi) * inv, (hi_ - oi) * inv);
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, t1))
}

/// Ray parameter at which the walk leaves the starting cell along one axis.
#[inline]
fn boundary_t(p: f32, d: f32, cell_index: i32, cell: f32, inv_d: f32, t_start: f32) -> f32 {
    if d.abs() < 1e-12 {
        return f32::MAX;
    }
    let edge = if d > 0.0 {
        (cell_index + 1) as f32 * cell
    } else {
        cell_index as f32 * cell
    };
    t_start + (edge - p) * inv_d
}
