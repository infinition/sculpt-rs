//! Dense triangle mesh with incremental vertex/face adjacency.
//!
//! Both `verts` and `faces` are kept gap-free so they can be handed to the GPU
//! without a compaction pass. Removal therefore uses `swap_remove` plus an index
//! fixup for the element that got moved into the hole.
//!
//! The mesh also owns an optional spatial grid (see [`crate::accel`]). Every
//! mutation that goes through the methods here keeps it in sync; anything that
//! rewrites the arrays wholesale drops it and lets the next query rebuild.

use crate::accel::Grid;
use glam::Vec3;
use rayon::prelude::*;
use smallvec::SmallVec;

/// Incident faces of a vertex. Regular sculpted meshes sit around valence 6.
pub type FaceList = SmallVec<[u32; 8]>;
/// Ring-1 neighbourhood of a vertex.
pub type VertList = SmallVec<[u32; 12]>;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: Vec3,
    pub nrm: Vec3,
    pub col: Vec3,
    /// 0 = freely sculptable, 1 = fully protected.
    pub mask: f32,
    /// PBR roughness, painted like colour.
    pub rough: f32,
    /// PBR metalness.
    pub metal: f32,
}

impl Vertex {
    pub fn new(pos: Vec3) -> Self {
        Self {
            pos,
            nrm: Vec3::Y,
            col: Vec3::splat(0.85),
            mask: 0.0,
            rough: 0.6,
            metal: 0.0,
        }
    }

    /// Linear blend of everything but the position, used when a new vertex is
    /// born between two existing ones.
    pub fn lerp_attrs(a: &Vertex, b: &Vertex, t: f32) -> Vertex {
        Vertex {
            pos: a.pos.lerp(b.pos, t),
            nrm: a.nrm.lerp(b.nrm, t).normalize_or(Vec3::Y),
            col: a.col.lerp(b.col, t),
            mask: a.mask + (b.mask - a.mask) * t,
            rough: a.rough + (b.rough - a.rough) * t,
            metal: a.metal + (b.metal - a.metal) * t,
        }
    }
}

#[derive(Clone, Default)]
pub struct Mesh {
    pub verts: Vec<Vertex>,
    pub faces: Vec<[u32; 3]>,
    /// `vfaces[v]` lists every face index referencing `v`.
    pub vfaces: Vec<FaceList>,
    /// Spatial index. `None` means "not built yet or invalidated".
    pub accel: Option<Grid>,
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a mesh from a raw triangle soup and derives adjacency + normals.
    pub fn from_soup(positions: &[Vec3], faces: &[[u32; 3]]) -> Self {
        let mut m = Self {
            verts: positions.iter().map(|p| Vertex::new(*p)).collect(),
            faces: faces.to_vec(),
            vfaces: vec![FaceList::new(); positions.len()],
            accel: None,
        };
        m.rebuild_adjacency();
        m.recompute_normals();
        m
    }

    pub fn vert_count(&self) -> usize {
        self.verts.len()
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    pub fn rebuild_adjacency(&mut self) {
        self.vfaces.clear();
        self.vfaces.resize(self.verts.len(), FaceList::new());
        for (fi, tri) in self.faces.iter().enumerate() {
            for &v in tri {
                self.vfaces[v as usize].push(fi as u32);
            }
        }
        self.accel = None;
    }

    // ---- spatial index ------------------------------------------------------

    /// Makes sure a grid exists and is scaled for queries of about `radius`.
    pub fn ensure_accel(&mut self, radius: f32) {
        let rebuild = match &self.accel {
            None => true,
            Some(g) => g.wants_rebuild(radius),
        };
        if rebuild {
            self.accel = Some(Grid::build(self, crate::accel::ideal_cell(radius)));
        }
    }

    pub fn invalidate_accel(&mut self) {
        self.accel = None;
    }

    #[inline]
    pub fn face_sphere(&self, f: u32) -> (Vec3, f32) {
        let [a, b, c] = self.faces[f as usize];
        let pa = self.verts[a as usize].pos;
        let pb = self.verts[b as usize].pos;
        let pc = self.verts[c as usize].pos;
        let mid = (pa + pb + pc) / 3.0;
        (mid, pa.distance(mid).max(pb.distance(mid)).max(pc.distance(mid)))
    }

    /// Refiles a vertex and, when it changed cell, its incident faces.
    fn refile(&mut self, v: u32) {
        if self.accel.is_none() {
            return;
        }
        let p = self.verts[v as usize].pos;
        let moved = self.accel.as_mut().map(|g| g.move_vertex(v, p)).unwrap_or(false);
        if !moved {
            return;
        }
        let faces: FaceList = self.vfaces[v as usize].clone();
        for f in faces {
            let (c, r) = self.face_sphere(f);
            if let Some(g) = &mut self.accel {
                g.move_face(f, c, r);
            }
        }
    }

    /// Moves one vertex, keeping the spatial index correct.
    pub fn set_pos(&mut self, v: u32, p: Vec3) {
        self.verts[v as usize].pos = p;
        self.refile(v);
    }

    /// Tells the index that these vertices were written to directly. Brushes
    /// batch their writes and call this once, which is much cheaper than going
    /// through [`Mesh::set_pos`] per vertex.
    pub fn commit_moves(&mut self, moved: &[u32]) {
        if self.accel.is_none() {
            return;
        }
        for &v in moved {
            self.refile(v);
        }
    }

    // ---- topology -----------------------------------------------------------

    pub fn add_vertex(&mut self, v: Vertex) -> u32 {
        let id = self.verts.len() as u32;
        if let Some(g) = &mut self.accel {
            g.push_vertex(id, v.pos);
        }
        self.verts.push(v);
        self.vfaces.push(FaceList::new());
        id
    }

    pub fn add_face(&mut self, tri: [u32; 3]) -> u32 {
        let fi = self.faces.len() as u32;
        self.faces.push(tri);
        for &v in &tri {
            self.vfaces[v as usize].push(fi);
        }
        if self.accel.is_some() {
            let (c, r) = self.face_sphere(fi);
            if let Some(g) = &mut self.accel {
                g.push_face(fi, c, r);
            }
        }
        fi
    }

    /// Removes a face, moving the last face into its slot.
    pub fn remove_face(&mut self, f: u32) {
        let fi = f as usize;
        let tri = self.faces[fi];
        for &v in &tri {
            // SmallVec hands out `&mut T` here, unlike Vec::retain.
            self.vfaces[v as usize].retain(|x| *x != f);
        }
        let last = self.faces.len() - 1;
        if fi != last {
            let moved = self.faces[last];
            for &v in &moved {
                for e in self.vfaces[v as usize].iter_mut() {
                    if *e == last as u32 {
                        *e = f;
                    }
                }
            }
        }
        if let Some(g) = &mut self.accel {
            g.swap_remove_face(f);
        }
        self.faces.swap_remove(fi);
    }

    /// Removes an isolated vertex, moving the last vertex into its slot.
    /// The caller must have removed every incident face first.
    pub fn remove_vertex(&mut self, v: u32) {
        let vi = v as usize;
        debug_assert!(self.vfaces[vi].is_empty(), "remove_vertex on a used vertex");
        let last = self.verts.len() - 1;
        if vi != last {
            let moved: FaceList = self.vfaces[last].clone();
            for f in moved {
                for x in self.faces[f as usize].iter_mut() {
                    if *x == last as u32 {
                        *x = v;
                    }
                }
            }
        }
        if let Some(g) = &mut self.accel {
            g.swap_remove_vertex(v);
        }
        self.verts.swap_remove(vi);
        self.vfaces.swap_remove(vi);
    }

    /// The one or two faces sharing edge `(a, b)`.
    pub fn faces_around_edge(&self, a: u32, b: u32) -> SmallVec<[u32; 2]> {
        let mut out = SmallVec::new();
        for &f in &self.vfaces[a as usize] {
            if self.faces[f as usize].contains(&b) {
                out.push(f);
            }
        }
        out
    }

    pub fn neighbors(&self, v: u32) -> VertList {
        let mut out = VertList::new();
        for &f in &self.vfaces[v as usize] {
            for &x in &self.faces[f as usize] {
                if x != v && !out.contains(&x) {
                    out.push(x);
                }
            }
        }
        out
    }

    pub fn is_boundary_vertex(&self, v: u32) -> bool {
        for &f in &self.vfaces[v as usize] {
            let tri = self.faces[f as usize];
            let i = tri.iter().position(|&x| x == v).unwrap();
            let nb = tri[(i + 1) % 3];
            if self.faces_around_edge(v, nb).len() < 2 {
                return true;
            }
        }
        false
    }

    pub fn edge_len(&self, a: u32, b: u32) -> f32 {
        self.verts[a as usize].pos.distance(self.verts[b as usize].pos)
    }

    pub fn face_normal(&self, f: u32) -> Vec3 {
        let [a, b, c] = self.faces[f as usize];
        let pa = self.verts[a as usize].pos;
        let pb = self.verts[b as usize].pos;
        let pc = self.verts[c as usize].pos;
        (pb - pa).cross(pc - pa)
    }

    /// Splits edge `(a, b)` at its midpoint, returning the new vertex.
    /// Winding of the affected faces is preserved.
    pub fn split_edge(&mut self, a: u32, b: u32) -> Option<u32> {
        let fs = self.faces_around_edge(a, b);
        if fs.is_empty() {
            return None;
        }
        let mid = Vertex::lerp_attrs(&self.verts[a as usize], &self.verts[b as usize], 0.5);
        let m = self.add_vertex(mid);

        // Collect first: remove_face invalidates the indices in `fs`.
        let tris: SmallVec<[[u32; 3]; 2]> = fs.iter().map(|&f| self.faces[f as usize]).collect();
        let mut sorted: SmallVec<[u32; 2]> = fs;
        sorted.sort_unstable_by(|x, y| y.cmp(x));
        for f in sorted {
            self.remove_face(f);
        }

        for tri in tris {
            let i = tri.iter().position(|&x| x == a).unwrap();
            let o;
            if tri[(i + 1) % 3] == b {
                // winding reads (a, b, o)
                o = tri[(i + 2) % 3];
                self.add_face([a, m, o]);
                self.add_face([m, b, o]);
            } else {
                // winding reads (a, o, b)
                o = tri[(i + 1) % 3];
                self.add_face([a, o, m]);
                self.add_face([m, o, b]);
            }
        }
        Some(m)
    }

    /// Collapses `b` into `a` at the edge midpoint.
    ///
    /// Rejects the collapse when it would break manifoldness (link condition)
    /// or flip a face normal by more than ~90 degrees.
    pub fn collapse_edge(&mut self, a: u32, b: u32) -> bool {
        if a == b {
            return false;
        }
        let fs = self.faces_around_edge(a, b);
        if fs.len() != 2 {
            return false; // boundary or non-manifold edge, leave it alone
        }
        // Link condition: the neighbourhoods of a and b may only share the two
        // vertices opposite the edge, otherwise the collapse creates a fin.
        let na = self.neighbors(a);
        let nb = self.neighbors(b);
        let shared = na.iter().filter(|x| nb.contains(x)).count();
        if shared != 2 {
            return false;
        }
        if self.is_boundary_vertex(a) || self.is_boundary_vertex(b) {
            return false;
        }

        let mid = (self.verts[a as usize].pos + self.verts[b as usize].pos) * 0.5;

        // Reject if any surviving face would flip.
        for &f in self.vfaces[a as usize].iter().chain(self.vfaces[b as usize].iter()) {
            if fs.contains(&f) {
                continue;
            }
            let tri = self.faces[f as usize];
            let before = self.face_normal(f);
            let p: [Vec3; 3] = std::array::from_fn(|k| {
                let v = tri[k];
                if v == a || v == b { mid } else { self.verts[v as usize].pos }
            });
            let after = (p[1] - p[0]).cross(p[2] - p[0]);
            if before.dot(after) <= 0.0 {
                return false;
            }
        }

        let merged = Vertex::lerp_attrs(&self.verts[a as usize], &self.verts[b as usize], 0.5);
        self.verts[a as usize] = merged;

        let mut doomed: SmallVec<[u32; 2]> = fs;
        doomed.sort_unstable_by(|x, y| y.cmp(x));
        for f in doomed {
            self.remove_face(f);
        }

        // Retarget b's remaining faces onto a.
        let rest: FaceList = self.vfaces[b as usize].clone();
        for f in rest {
            for x in self.faces[f as usize].iter_mut() {
                if *x == b {
                    *x = a;
                }
            }
            self.vfaces[a as usize].push(f);
        }
        self.vfaces[b as usize].clear();
        self.remove_vertex(b);
        // `a` moved to the midpoint and inherited faces: refile everything it
        // now touches.
        self.refile(a);
        if self.accel.is_some() {
            let faces: FaceList = self.vfaces[a as usize].clone();
            for f in faces {
                let (c, r) = self.face_sphere(f);
                if let Some(g) = &mut self.accel {
                    g.move_face(f, c, r);
                }
            }
        }
        true
    }

    // ---- derived data -------------------------------------------------------

    /// Area-weighted normals over the whole mesh.
    pub fn recompute_normals(&mut self) {
        let faces = &self.faces;
        let verts = &self.verts;
        let normals: Vec<Vec3> = self
            .vfaces
            .par_iter()
            .map(|fl| {
                let mut n = Vec3::ZERO;
                for &f in fl {
                    let [a, b, c] = faces[f as usize];
                    let pa = verts[a as usize].pos;
                    let pb = verts[b as usize].pos;
                    let pc = verts[c as usize].pos;
                    n += (pb - pa).cross(pc - pa);
                }
                n.normalize_or(Vec3::Y)
            })
            .collect();
        for (v, n) in self.verts.iter_mut().zip(normals) {
            v.nrm = n;
        }
    }

    /// Recomputes normals for `verts` and their ring-1 neighbours only.
    ///
    /// Returns that wider set, which is exactly the set of vertices whose data
    /// changed and therefore what a renderer has to re-upload.
    pub fn update_normals(&mut self, touched: &[u32]) -> Vec<u32> {
        let mut set: Vec<u32> = Vec::with_capacity(touched.len() * 7);
        set.extend_from_slice(touched);
        for &v in touched {
            set.extend(self.neighbors(v));
        }
        set.sort_unstable();
        set.dedup();
        for &v in &set {
            let mut n = Vec3::ZERO;
            for &f in &self.vfaces[v as usize] {
                n += self.face_normal(f);
            }
            self.verts[v as usize].nrm = n.normalize_or(Vec3::Y);
        }
        set
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        if self.verts.is_empty() {
            return (Vec3::ZERO, Vec3::ZERO);
        }
        let (lo, hi) = self
            .verts
            .par_iter()
            .map(|v| (v.pos, v.pos))
            .reduce(
                || (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                |a, b| (a.0.min(b.0), a.1.max(b.1)),
            );
        (lo, hi)
    }

    pub fn center(&self) -> Vec3 {
        let (lo, hi) = self.bounds();
        (lo + hi) * 0.5
    }

    /// Centres the mesh on the origin and scales it to fit a unit sphere.
    pub fn normalize_scale(&mut self) {
        let (lo, hi) = self.bounds();
        let c = (lo + hi) * 0.5;
        let r = (hi - lo).max_element() * 0.5;
        if r <= 0.0 {
            return;
        }
        let s = 1.0 / r;
        self.verts.par_iter_mut().for_each(|v| v.pos = (v.pos - c) * s);
        self.invalidate_accel();
    }

    /// Applies an arbitrary affine transform, fixing up normals and winding.
    pub fn apply_transform(&mut self, m: glam::Mat4) {
        let normal_mat = glam::Mat3::from_mat4(m).inverse().transpose();
        self.verts.par_iter_mut().for_each(|v| {
            v.pos = m.transform_point3(v.pos);
            v.nrm = (normal_mat * v.nrm).normalize_or(Vec3::Y);
        });
        if glam::Mat3::from_mat4(m).determinant() < 0.0 {
            self.faces.par_iter_mut().for_each(|t| t.swap(1, 2));
            self.rebuild_adjacency();
        }
        self.invalidate_accel();
    }

    /// Mean edge length, used to seed the dyntopo detail size.
    pub fn mean_edge_len(&self) -> f32 {
        if self.faces.is_empty() {
            return 0.1;
        }
        let sum: f32 = self
            .faces
            .par_iter()
            .map(|&[a, b, c]| {
                let pa = self.verts[a as usize].pos;
                let pb = self.verts[b as usize].pos;
                let pc = self.verts[c as usize].pos;
                pa.distance(pb) + pb.distance(pc) + pc.distance(pa)
            })
            .sum();
        sum / (self.faces.len() as f32 * 3.0)
    }

    /// Total surface area.
    pub fn area(&self) -> f32 {
        self.faces
            .par_iter()
            .enumerate()
            .map(|(i, _)| self.face_normal(i as u32).length() * 0.5)
            .sum()
    }

    /// Boundary edges as directed pairs, each appearing once, oriented so the
    /// hole is on the left. Used by hole filling.
    pub fn boundary_edges(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for (fi, tri) in self.faces.iter().enumerate() {
            let _ = fi;
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                if self.faces_around_edge(a, b).len() == 1 {
                    out.push((a, b));
                }
            }
        }
        out
    }

    /// Closed loops of boundary edges, longest first.
    pub fn boundary_loops(&self) -> Vec<Vec<u32>> {
        let edges = self.boundary_edges();
        let mut next: rustc_hash::FxHashMap<u32, u32> = rustc_hash::FxHashMap::default();
        for (a, b) in &edges {
            next.insert(*a, *b);
        }
        let mut loops = Vec::new();
        let mut seen: rustc_hash::FxHashSet<u32> = rustc_hash::FxHashSet::default();
        for (start, _) in &edges {
            if seen.contains(start) {
                continue;
            }
            let mut chain = Vec::new();
            let mut cur = *start;
            while seen.insert(cur) {
                chain.push(cur);
                match next.get(&cur) {
                    Some(&n) => cur = n,
                    None => break,
                }
                if cur == *start {
                    break;
                }
            }
            if chain.len() >= 3 {
                loops.push(chain);
            }
        }
        loops.sort_by_key(|l| std::cmp::Reverse(l.len()));
        loops
    }
}
