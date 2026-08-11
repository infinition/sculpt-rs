//! Sparse, chunked implicit surface, the sculptable truth.
//!
//! A triangle mesh is a terrible thing to sculpt directly: a dab costs what
//! the tessellation under the brush costs, so a five-million-triangle model
//! sculpts at a fraction of the speed of a one-million one, and every dab adds
//! the same penalty again. This module keeps the surface as a scalar field,
//! stored sparsely in fixed-size chunks that only exist near the surface. A
//! dab touches a fixed number of voxels whatever the mesh it was voxelised
//! from, so the cost of sculpting stops depending on the polygon count.
//!
//! The field is an implicit function, positive outside the surface and
//! negative inside, with the surface at zero. A chunk that does not exist
//! reads as far outside. The surface is extracted back into triangles with
//! surface nets, which place a vertex in every cell whose edges cross the
//! surface and connect the cells around each crossing edge into quads.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};

use glam::Vec3;
use rayon::prelude::*;

use crate::mesh::{Mesh, Vertex};
use crate::topology::point_triangle_distance;

/// Voxels per chunk side.
///
/// Sixteen makes a chunk a 16 KiB block, and the index math is a shift and a
/// mask.
pub const CHUNK: usize = 16;
const CHUNK_I32: i32 = CHUNK as i32;

type Chunk = [f32; CHUNK * CHUNK * CHUNK];

/// A value read from a chunk that does not exist: far outside the surface, so
/// far that no edge touching it can be near a crossing on its own.
const OUTSIDE: f32 = 1e6;

/// One surface cell, keyed by its lower corner's voxel index.
type Cell = (i32, i32, i32);

/// The sparse, chunked field.
#[derive(Clone)]
pub struct VoxelField {
    /// World units per voxel.
    pub h: f32,
    /// World position of voxel (0, 0, 0)'s corner.
    pub origin: Vec3,
    chunks: HashMap<(i32, i32, i32), Box<Chunk>>,
    /// Chunks written since the last extraction, so only they are re-extracted.
    dirty: HashSet<(i32, i32, i32)>,
    /// Extracted surface vertex per cell.
    cell_pos: HashMap<Cell, Vec3>,
    /// Triangles of each cell, named by the cells they join rather than by
    /// mesh indices, so a cell can be re-extracted without knowing the mesh.
    cell_quads: HashMap<Cell, Vec<[Cell; 3]>>,
    /// The surface as a triangle mesh, rebuilt from the caches when a dirty
    /// chunk is re-extracted.
    surface: Mesh,
}

fn flat(i: usize, j: usize, k: usize) -> usize {
    (k * CHUNK + j) * CHUNK + i
}

impl VoxelField {
    /// An empty field at the given voxel size.
    pub fn new(h: f32) -> Self {
        Self {
            h: h.max(1e-6),
            origin: Vec3::ZERO,
            chunks: HashMap::new(),
            dirty: HashSet::new(),
            cell_pos: HashMap::new(),
            cell_quads: HashMap::new(),
            surface: Mesh::new(),
        }
    }

    /// Voxelises a triangle mesh into the field, at voxel size `h`.
    ///
    /// This is the one cost that does depend on the mesh: every triangle is
    /// walked into the band around it. It happens once, on load, and after it
    /// the sculpting cost depends only on the voxel size, never again on the
    /// polygon count.
    pub fn from_mesh(mesh: &Mesh, h: f32) -> Self {
        let h = h.max(1e-6);
        let (lo, hi) = mesh.bounds();
        let extent = hi - lo;
        let pad = Vec3::splat(h * 4.0);
        let origin = lo - pad;
        let size = extent + pad * 2.0;
        let nx = ((size.x / h).ceil() as usize + 2).clamp(4, 300);
        let ny = ((size.y / h).ceil() as usize + 2).clamp(4, 300);
        let nz = ((size.z / h).ceil() as usize + 2).clamp(4, 300);
        let total = nx * ny * nz;
        let band = h * 2.5;
        let idx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;

        // Unsigned distance inside a narrow band around every triangle. The
        // minimum is an atomic op because triangles overlap in the band.
        let mut field: Vec<f32> = {
            let cells: Vec<AtomicU32> = (0..total)
                .map(|_| AtomicU32::new(band.to_bits()))
                .collect();
            mesh.faces.par_iter().for_each(|tri| {
                let a = mesh.pos[tri[0] as usize];
                let b = mesh.pos[tri[1] as usize];
                let c = mesh.pos[tri[2] as usize];
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
                            cells[idx(i, j, k)].fetch_min(d.to_bits(), Ordering::Relaxed);
                        }
                    }
                }
            });
            cells.into_par_iter().map(|c| f32::from_bits(c.into_inner())).collect()
        };

        // Sign by parity along X: rasterise every triangle onto each grid line
        // and flip each voxel the line crosses.
        let mut crossings: Vec<Vec<f32>> = vec![Vec::new(); ny * nz];
        let per_part = (ny * nz).div_ceil(rayon::current_num_threads().max(1)).max(1);
        crossings
            .par_chunks_mut(per_part)
            .enumerate()
            .for_each(|(part, mine)| {
                let first = part * per_part;
                let last = first + mine.len();
                for tri in &mesh.faces {
                    let a = mesh.pos[tri[0] as usize];
                    let b = mesh.pos[tri[1] as usize];
                    let c = mesh.pos[tri[2] as usize];
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
                    if k1 * ny + j1 < first || k0 * ny + j0 >= last {
                        continue;
                    }
                    let d = (b.y - a.y) * (c.z - a.z) - (c.y - a.y) * (b.z - a.z);
                    if d.abs() < 1e-20 {
                        continue;
                    }
                    let inv_d = 1.0 / d;
                    for k in k0..=k1 {
                        let pz = origin.z + k as f32 * h;
                        for j in j0..=j1 {
                            let line = k * ny + j;
                            if line < first || line >= last {
                                continue;
                            }
                            let py = origin.y + j as f32 * h;
                            let u = ((py - a.y) * (c.z - a.z) - (c.y - a.y) * (pz - a.z)) * inv_d;
                            let v = ((b.y - a.y) * (pz - a.z) - (py - a.y) * (b.z - a.z)) * inv_d;
                            if u < 0.0 || v < 0.0 || u + v > 1.0 {
                                continue;
                            }
                            let x = a.x + u * (b.x - a.x) + v * (c.x - a.x);
                            mine[line - first].push(x);
                        }
                    }
                }
            });
        field
            .par_chunks_mut(nx)
            .zip(crossings.par_iter_mut())
            .for_each(|(row, xs)| {
                if xs.is_empty() {
                    return;
                }
                xs.sort_unstable_by(|p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
                let mut cursor = 0usize;
                let mut inside = false;
                for (i, slot) in row.iter_mut().enumerate() {
                    let x = origin.x + i as f32 * h;
                    while cursor < xs.len() && xs[cursor] <= x {
                        inside = !inside;
                        cursor += 1;
                    }
                    if inside {
                        *slot = -*slot;
                    }
                }
            });

        // Hand the dense band over to the sparse field, keeping only the voxels
        // close enough to the surface for the extraction to read them.
        let mut out = Self::new(h);
        out.origin = origin;
        for i in 0..nx {
            for j in 0..ny {
                for k in 0..nz {
                    let d = field[idx(i, j, k)];
                    if d.abs() < h * 4.0 {
                        out.write_value((i as i32, j as i32, k as i32), d);
                    }
                }
            }
        }
        out
    }

    /// An analytic sphere, the quickest way to stand the machinery up.
    ///
    /// Only the band around the surface is stored, which is what makes the
    /// field sparse: a model that fills a million-voxel box keeps the few
    /// thousand that the surface actually crosses.
    pub fn sphere(center: Vec3, radius: f32, h: f32) -> Self {
        let mut f = Self::new(h);
        let half = (radius / h).ceil() as i32 + 4;
        let lo = center - Vec3::splat(half as f32 * h);
        f.origin = lo;
        for di in 0..(2 * half) {
            for dj in 0..(2 * half) {
                for dk in 0..(2 * half) {
                    let p = lo + Vec3::new((di as f32 + 0.5) * h, (dj as f32 + 0.5) * h, (dk as f32 + 0.5) * h);
                    let d = p.distance(center) - radius;
                    if d.abs() < h * 4.0 {
                        f.write_value((di, dj, dk), d);
                    }
                }
            }
        }
        f
    }

    /// Writes one voxel value, allocating its chunk on demand.
    fn write_value(&mut self, (i, j, k): (i32, i32, i32), v: f32) {
        let (cx, cy, cz) = (
            i.div_euclid(CHUNK_I32),
            j.div_euclid(CHUNK_I32),
            k.div_euclid(CHUNK_I32),
        );
        let (li, lj, lk) = (
            i.rem_euclid(CHUNK_I32) as usize,
            j.rem_euclid(CHUNK_I32) as usize,
            k.rem_euclid(CHUNK_I32) as usize,
        );
        let chunk = self
            .chunks
            .entry((cx, cy, cz))
            .or_insert_with(|| Box::new([OUTSIDE; CHUNK * CHUNK * CHUNK]));
        chunk[flat(li, lj, lk)] = v;
    }

    /// The value at a voxel, or far outside when its chunk does not exist.
    fn voxel(&self, i: i32, j: i32, k: i32) -> f32 {
        let (cx, cy, cz) = (
            i.div_euclid(CHUNK_I32),
            j.div_euclid(CHUNK_I32),
            k.div_euclid(CHUNK_I32),
        );
        let (li, lj, lk) = (
            i.rem_euclid(CHUNK_I32) as usize,
            j.rem_euclid(CHUNK_I32) as usize,
            k.rem_euclid(CHUNK_I32) as usize,
        );
        match self.chunks.get(&(cx, cy, cz)) {
            Some(c) => c[flat(li, lj, lk)],
            None => OUTSIDE,
        }
    }

    fn inside(&self, i: i32, j: i32, k: i32) -> bool {
        self.voxel(i, j, k) < 0.0
    }

    /// True when the voxel lives in a stored chunk. A cell whose corners are
    /// not all stored is beyond the reliable band, and a corner read from a
    /// missing chunk would guess its sign, which is what draws a phantom shell
    /// around the edge of the stored region.
    fn stored(&self, i: i32, j: i32, k: i32) -> bool {
        self.chunks.contains_key(&(
            i.div_euclid(CHUNK_I32),
            j.div_euclid(CHUNK_I32),
            k.div_euclid(CHUNK_I32),
        ))
    }

    /// The world position of a voxel's corner.
    fn corner(&self, i: i32, j: i32, k: i32) -> Vec3 {
        self.origin + Vec3::new(i as f32, j as f32, k as f32) * self.h
    }

    /// Applies a brush dab: adds a smooth bump of `amount` (positive pushes
    /// the surface out, negative carves) inside `radius` around `center`.
    ///
    /// The cost is the number of voxels the radius covers, which depends only
    /// on the radius and the voxel size, never on how the surface was made.
    pub fn stamp(&mut self, center: Vec3, radius: f32, amount: f32) {
        let r = (radius / self.h).ceil() as i32 + 1;
        let (ci, cj, ck) = (
            ((center.x - self.origin.x) / self.h).floor() as i32,
            ((center.y - self.origin.y) / self.h).floor() as i32,
            ((center.z - self.origin.z) / self.h).floor() as i32,
        );
        let inv_r = 1.0 / radius.max(1e-6);
        for di in -r..=r {
            for dj in -r..=r {
                for dk in -r..=r {
                    let p = self.corner(ci + di, cj + dj, ck + dk);
                    let d = p.distance(center);
                    if d > radius {
                        continue;
                    }
                    // A smooth bump that vanishes at the edge of the brush,
                    // so the surface it leaves behind has no seam.
                    let t = d * inv_r;
                    let w = (1.0 - t * t).powi(2);
                    let old = self.voxel(ci + di, cj + dj, ck + dk);
                    self.write_value((ci + di, cj + dj, ck + dk), old - amount * w);
                }
            }
        }
    }

    /// How many voxels a stamp of `radius` touches, for a readout of the cost.
    pub fn stamp_cells(&self, radius: f32) -> usize {
        let r = (radius / self.h).ceil() as i32 + 1;
        (2 * r + 1).pow(3) as usize
    }

    /// How many chunks the field holds, a readout of how sparse it is.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Extracts the whole surface, recomputing every cell.
    ///
    /// What a freshly voxelised field wants. After it, [`Self::extract_modified`]
    /// keeps the surface up to date by re-extracting only the written chunks.
    pub fn extract(&mut self) -> Mesh {
        self.cell_pos.clear();
        self.cell_quads.clear();
        let cells: Vec<Cell> = self
            .chunks
            .keys()
            .flat_map(|&(cx, cy, cz)| {
                (0..CHUNK).flat_map(move |lk| {
                    (0..CHUNK)
                        .flat_map(move |lj| {
                            (0..CHUNK).map(move |li| {
                                (
                                    cx * CHUNK_I32 + li as i32,
                                    cy * CHUNK_I32 + lj as i32,
                                    cz * CHUNK_I32 + lk as i32,
                                )
                            })
                        })
                })
            })
            .collect();
        // Positions first, then faces: a quad names its four cells, and they
        // must all exist before any of them is wired.
        for &cell in &cells {
            self.compute_cell_pos(cell);
        }
        for &cell in &cells {
            self.compute_cell_quads(cell);
        }
        self.rebuild();
        self.dirty.clear();
        self.surface.clone()
    }

    /// Re-extracts only the cells the written chunks and their halo cover,
    /// and rebuilds the surface from the caches.
    ///
    /// This is the per-dab cost. A stamp writes a handful of chunks, and only
    /// their cells and the band around them are re-examined; the rest of the
    /// surface is handed back as it was.
    pub fn extract_modified(&mut self) -> Mesh {
        if self.dirty.is_empty() {
            return self.surface.clone();
        }
        let dirty: Vec<(i32, i32, i32)> = self.dirty.iter().copied().collect();
        // The halo is three cells: one for the corner values a cell reads from
        // its neighbours, and two more for the quads whose four cells straddle
        // the chunk boundary.
        let mut affected: Vec<Cell> = Vec::new();
        for &(cx, cy, cz) in &dirty {
            for i in cx * CHUNK_I32 - 3..=(cx + 1) * CHUNK_I32 + 2 {
                for j in cy * CHUNK_I32 - 3..=(cy + 1) * CHUNK_I32 + 2 {
                    for k in cz * CHUNK_I32 - 3..=(cz + 1) * CHUNK_I32 + 2 {
                        affected.push((i, j, k));
                    }
                }
            }
        }
        for &cell in &affected {
            self.compute_cell_pos(cell);
        }
        for &cell in &affected {
            self.compute_cell_quads(cell);
        }
        self.rebuild();
        self.dirty.clear();
        self.surface.clone()
    }

    /// Rebuilds the surface mesh from the cached cells.
    fn rebuild(&mut self) {
        let mut mesh = Mesh::new();
        let mut index: HashMap<Cell, u32> = HashMap::with_capacity(self.cell_pos.len());
        for (cell, &pos) in &self.cell_pos {
            let vi = mesh.add_vertex(Vertex::new(pos));
            index.insert(*cell, vi);
        }
        for tris in self.cell_quads.values() {
            for tri in tris {
                let Some(a) = index.get(&tri[0]).copied() else { continue };
                let Some(b) = index.get(&tri[1]).copied() else { continue };
                let Some(c) = index.get(&tri[2]).copied() else { continue };
                mesh.add_face([a, b, c]);
            }
        }
        self.surface = mesh;
    }

    /// Recomputes one cell's surface vertex into the cache.
    fn compute_cell_pos(&mut self, cell: Cell) {
        match self.cell_vertex(cell.0, cell.1, cell.2) {
            Some(p) => self.cell_pos.insert(cell, p),
            None => self.cell_pos.remove(&cell),
        };
    }

    /// Regenerates one cell's triangles, read from the cached vertices, which
    /// all exist by now: the positions are computed in their own pass first,
    /// or a quad would drop because a neighbour it names had not been reached.
    fn compute_cell_quads(&mut self, cell: Cell) {
        let quads = self.cell_faces_from(cell);
        self.cell_quads.insert(cell, quads);
    }

    /// The up-to-two triangles a cell contributes, named by the cells they
    /// join, from its three positive edges.
    fn cell_faces_from(&self, cell: Cell) -> Vec<[Cell; 3]> {
        let (i, j, k) = cell;
        let mut out = Vec::new();
        self.quad_along_x(&mut out, i, j, k);
        self.quad_along_y(&mut out, i, j, k);
        self.quad_along_z(&mut out, i, j, k);
        out
    }

    /// The mean crossing point of a cell, or `None` when the surface misses it.
    fn cell_vertex(&self, i: i32, j: i32, k: i32) -> Option<Vec3> {
        // The 12 edges of the cell, as pairs of corners.
        const EDGES: [(i32, i32, i32, i32, i32, i32); 12] = [
            // along X
            (0, 0, 0, 1, 0, 0),
            (0, 1, 0, 1, 1, 0),
            (0, 0, 1, 1, 0, 1),
            (0, 1, 1, 1, 1, 1),
            // along Y
            (0, 0, 0, 0, 1, 0),
            (1, 0, 0, 1, 1, 0),
            (0, 0, 1, 0, 1, 1),
            (1, 0, 1, 1, 1, 1),
            // along Z
            (0, 0, 0, 0, 0, 1),
            (1, 0, 0, 1, 0, 1),
            (0, 1, 0, 0, 1, 1),
            (1, 1, 0, 1, 1, 1),
        ];
        // Only a cell entirely inside the stored band is extracted: a missing
        // corner would be a guess at the sign, and a guess is a phantom shell.
        if !(self.stored(i, j, k)
            && self.stored(i + 1, j, k)
            && self.stored(i, j + 1, k)
            && self.stored(i, j, k + 1)
            && self.stored(i + 1, j + 1, k)
            && self.stored(i + 1, j, k + 1)
            && self.stored(i, j + 1, k + 1)
            && self.stored(i + 1, j + 1, k + 1))
        {
            return None;
        }
        let mut sum = Vec3::ZERO;
        let mut count = 0usize;
        for (ax, ay, az, bx, by, bz) in EDGES {
            let (ia, ja, ka) = (i + ax, j + ay, k + az);
            let (ib, jb, kb) = (i + bx, j + by, k + bz);
            if self.inside(ia, ja, ka) == self.inside(ib, jb, kb) {
                continue;
            }
            let va = self.voxel(ia, ja, ka);
            let vb = self.voxel(ib, jb, kb);
            let t = va / (va - vb);
            let pa = self.corner(ia, ja, ka);
            let pb = self.corner(ib, jb, kb);
            sum += pa + (pb - pa) * t;
            count += 1;
        }
        if count == 0 {
            None
        } else {
            Some(sum / count as f32)
        }
    }

    /// Pushes a quad for the X edge of cell (i, j, k), if it crosses.
    fn quad_along_x(&self, out: &mut Vec<[Cell; 3]>, i: i32, j: i32, k: i32) {
        if self.inside(i, j, k) == self.inside(i + 1, j, k) {
            return;
        }
        // The four cells sharing this edge: it lies along the X axis at
        // (i, j, k), so the ring is the cells offset in y and z around it.
        let ring = [(i, j, k), (i, j - 1, k), (i, j - 1, k - 1), (i, j, k - 1)];
        self.push_quad(out, ring);
    }

    fn quad_along_y(&self, out: &mut Vec<[Cell; 3]>, i: i32, j: i32, k: i32) {
        if self.inside(i, j, k) == self.inside(i, j + 1, k) {
            return;
        }
        let ring = [(i, j, k), (i - 1, j, k), (i - 1, j, k - 1), (i, j, k - 1)];
        self.push_quad(out, ring);
    }

    fn quad_along_z(&self, out: &mut Vec<[Cell; 3]>, i: i32, j: i32, k: i32) {
        if self.inside(i, j, k) == self.inside(i, j, k + 1) {
            return;
        }
        let ring = [(i, j, k), (i - 1, j, k), (i - 1, j - 1, k), (i, j - 1, k)];
        self.push_quad(out, ring);
    }

    fn push_quad(&self, out: &mut Vec<[Cell; 3]>, ring: [Cell; 4]) {
        // A quad needs all four cells to have a surface vertex. At the edge of
        // the stored field one is missing and the quad is dropped, which only
        // happens where the surface leaves the field.
        let [a, b, c, d] = ring;
        if !(self.cell_pos.contains_key(&a)
            && self.cell_pos.contains_key(&b)
            && self.cell_pos.contains_key(&c)
            && self.cell_pos.contains_key(&d))
        {
            return;
        }
        out.push([a, b, c]);
        out.push([a, c, d]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sphere_extracts_as_a_closed_shell() {
        let mut f = VoxelField::sphere(Vec3::ZERO, 1.0, 0.05);
        let mesh = f.extract();
        // A unit sphere at h = 0.05 has a surface of a few thousand cells.
        assert!(
            mesh.face_count() > 2_000,
            "sphere came out too small: {}",
            mesh.face_count()
        );
        assert!(
            mesh.face_count() < 60_000,
            "sphere came out too big: {}",
            mesh.face_count()
        );
        // Watertight: the surface is closed, so no edge is used by one face.
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in &mesh.faces {
            for k in 0..3 {
                let (a, b) = (tri[k].min(tri[(k + 1) % 3]), tri[k].max(tri[(k + 1) % 3]));
                *edges.entry((a, b)).or_insert(0) += 1;
            }
        }
        let boundary = edges.values().filter(|&&n| n == 1).count();
        assert_eq!(boundary, 0, "surface has {} open edges", boundary);
    }

    #[test]
    fn a_stamp_pushes_the_surface_out() {
        let mut f = VoxelField::sphere(Vec3::ZERO, 1.0, 0.05);
        let before = f.extract().face_count();
        // Push clay out at the pole: the surface should grow.
        f.stamp(Vec3::new(0.0, 1.2, 0.0), 0.4, 0.3);
        let after = f.extract().face_count();
        assert!(after > before, "stamp did not grow the surface: {before} -> {after}");
    }

    #[test]
    fn a_carve_changes_the_surface_and_keeps_it_closed() {
        let mut f = VoxelField::sphere(Vec3::ZERO, 1.0, 0.05);
        let before = f.extract().face_count();
        // Carve into the pole, staying inside the stored band: the surface
        // recedes into a dent, and the dent is closed, not a hole.
        f.stamp(Vec3::new(0.0, 1.0, 0.0), 0.15, -0.25);
        let mesh = f.extract();
        assert_ne!(mesh.face_count(), before, "carve did not change the surface");
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in &mesh.faces {
            for k in 0..3 {
                let (a, b) = (tri[k].min(tri[(k + 1) % 3]), tri[k].max(tri[(k + 1) % 3]));
                *edges.entry((a, b)).or_insert(0) += 1;
            }
        }
        let boundary = edges.values().filter(|&&n| n == 1).count();
        assert_eq!(boundary, 0, "carve left {} open edges", boundary);
    }

    #[test]
    fn a_voxelised_mesh_extracts_as_a_closed_shell() {
        use crate::primitives;
        let mesh = primitives::icosphere(4);
        let mut f = VoxelField::from_mesh(&mesh, 0.04);
        let out = f.extract();
        assert!(out.face_count() > 1_000, "too small: {}", out.face_count());
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in &out.faces {
            for k in 0..3 {
                let (a, b) = (tri[k].min(tri[(k + 1) % 3]), tri[k].max(tri[(k + 1) % 3]));
                *edges.entry((a, b)).or_insert(0) += 1;
            }
        }
        let boundary = edges.values().filter(|&&n| n == 1).count();
        assert_eq!(boundary, 0, "shell has {} open edges", boundary);
    }

    #[test]
    fn the_stamp_cost_scales_with_voxel_size_not_surface() {
        let coarse = VoxelField::sphere(Vec3::ZERO, 1.0, 0.1);
        let fine = VoxelField::sphere(Vec3::ZERO, 1.0, 0.05);
        let coarse_cells = coarse.stamp_cells(0.3);
        let fine_cells = fine.stamp_cells(0.3);
        assert!(
            fine_cells > coarse_cells * 4,
            "fine {fine_cells} not coarser than coarse {coarse_cells}"
        );
    }
}
