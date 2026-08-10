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

/// Below this many elements, spreading the work costs more than doing it.
///
/// Handing a few hundred items to a thread pool is a synchronisation the
/// straight loop does not pay, and a laptop on battery has cores that take a
/// moment to wake. The threshold matters most on the machines with the fewest
/// cores, which are the ones that can least afford the overhead.
const PARALLEL_MIN: usize = 8192;

/// Fills the vertex-to-face lists, across every core.
///
/// Like the filing of the spatial grid, and for the same reason: the scatter
/// cannot be split by face, because two threads would push into the list of a
/// shared vertex. Split by vertex instead, and let each thread read the whole
/// face array keeping only what points into its own slice. A streaming read
/// per thread replaces a scatter over an array far too large for cache.
fn fill_adjacency(vfaces: &mut [FaceList], faces: &[[u32; 3]]) {
    let parts = rayon::current_num_threads();
    if faces.len() < PARALLEL_MIN || parts < 2 {
        for (fi, tri) in faces.iter().enumerate() {
            for &v in tri {
                vfaces[v as usize].push(fi as u32);
            }
        }
        return;
    }
    let parts = parts.min(3);
    let chunk = vfaces.len().div_ceil(parts).max(1);
    vfaces
        .par_chunks_mut(chunk)
        .enumerate()
        .for_each(|(k, part)| {
            let lo = (k * chunk) as u32;
            let hi = lo + part.len() as u32;
            for (fi, tri) in faces.iter().enumerate() {
                for &v in tri {
                    if v >= lo && v < hi {
                        part[(v - lo) as usize].push(fi as u32);
                    }
                }
            }
        });
}

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

/// What it would take to put a mesh back the way it was.
///
/// A stroke used to be remembered as a list of moved vertices, which is small
/// and exact right up to the moment dynamic topology cuts an edge: after that
/// the indices mean something else and the list is worthless. The fallback was
/// a copy of the whole mesh, taken on the first cut of every stroke, which on
/// five million triangles is a hundred and ninety megabytes and the pause that
/// goes with it, once per stroke.
///
/// This records the same thing a copy would, minus everything that did not
/// change. An array is fully described by its length and its contents, so
/// keeping the original value of every slot that was written or dropped, along
/// with the length it started at, is enough to rebuild it exactly. The cost
/// follows what the stroke touched rather than how large the model is.
#[derive(Clone, Debug)]
pub struct TopoLog {
    /// Vertex and face counts when the stroke began.
    pub vlen: usize,
    pub flen: usize,
    vseen: rustc_hash::FxHashSet<u32>,
    fseen: rustc_hash::FxHashSet<u32>,
    /// Slots as they were before the first write of this stroke reached them.
    pub verts: Vec<(u32, Vertex)>,
    pub faces: Vec<(u32, [u32; 3])>,
}

impl TopoLog {
    fn new(vlen: usize, flen: usize) -> Self {
        Self {
            vlen,
            flen,
            vseen: rustc_hash::FxHashSet::default(),
            fseen: rustc_hash::FxHashSet::default(),
            verts: Vec::new(),
            faces: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.verts.is_empty() && self.faces.is_empty()
    }

    /// Whether putting this back changes the shape of the arrays, rather than
    /// only what is in them.
    ///
    /// A stroke that only moved vertices can be undone without rebuilding the
    /// adjacency, which on a dense mesh is the difference between an instant
    /// undo and a noticeable one.
    pub fn structural(&self, vlen: usize, flen: usize) -> bool {
        !self.faces.is_empty() || self.vlen != vlen || self.flen != flen
    }

    pub fn bytes(&self) -> usize {
        self.verts.len() * (std::mem::size_of::<Vertex>() + 8)
            + self.faces.len() * (std::mem::size_of::<[u32; 3]>() + 8)
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
    /// Slots written since the last upload, so a renderer can send just those.
    ///
    /// Every mutation below records what it touched. This is the difference
    /// between sending a few kilobytes and re-sending a hundred megabyte mesh
    /// on every frame of a stroke. Duplicates are fine; the consumer sorts.
    pub dirty_verts: Vec<u32>,
    pub dirty_faces: Vec<u32>,
    /// Set by anything that rewrites the arrays wholesale, where per-slot
    /// tracking would be meaningless.
    pub fully_dirty: bool,
    /// What the stroke in progress has to put back, if one is being recorded.
    ///
    /// Driven through [`Mesh::begin_log`] and [`Mesh::take_log`] only: every
    /// mutation here keeps it honest by copying a slot before writing it, and
    /// a log filled in by hand would describe a mesh that never existed.
    pub(crate) log: Option<TopoLog>,
    /// Faces whose cell in the grid is out of date, waiting for
    /// [`Mesh::flush_refit`].
    pub(crate) stale_faces: Vec<u32>,
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
            ..Default::default()
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
        fill_adjacency(&mut self.vfaces, &self.faces);
        self.accel = None;
        self.mark_fully_dirty();
    }

    // ---- change tracking ----------------------------------------------------

    /// Declares that everything changed, which is what a wholesale rewrite of
    /// the arrays amounts to.
    pub fn mark_fully_dirty(&mut self) {
        self.fully_dirty = true;
        self.dirty_verts.clear();
        self.dirty_faces.clear();
        // A log describes how to walk back a set of slot writes. Once the
        // arrays have been rewritten from end to end it describes nothing, so
        // it is dropped rather than left to restore a mesh that no longer
        // exists. Nothing that lands here runs during a stroke.
        self.log = None;
    }

    // ---- undo log -----------------------------------------------------------

    /// Starts recording what it would take to undo what comes next.
    pub fn begin_log(&mut self) {
        self.log = Some(TopoLog::new(self.verts.len(), self.faces.len()));
    }

    /// Hands the recording over and stops recording.
    pub fn take_log(&mut self) -> Option<TopoLog> {
        self.log.take()
    }

    pub fn is_logging(&self) -> bool {
        self.log.is_some()
    }

    /// Keeps a vertex as it is now, before something writes over it.
    ///
    /// Must be called before the write, which is the whole contract. Slots past
    /// the length the stroke started at are ignored: undoing truncates them
    /// away, so what they held is of no interest.
    #[inline]
    fn log_vert(&mut self, v: u32) {
        let Some(log) = &mut self.log else { return };
        if (v as usize) < log.vlen && (v as usize) < self.verts.len() && log.vseen.insert(v) {
            log.verts.push((v, self.verts[v as usize]));
        }
    }

    #[inline]
    fn log_face(&mut self, f: u32) {
        let Some(log) = &mut self.log else { return };
        if (f as usize) < log.flen && (f as usize) < self.faces.len() && log.fseen.insert(f) {
            log.faces.push((f, self.faces[f as usize]));
        }
    }

    /// Keeps a batch of vertices, for a brush that is about to write into them
    /// directly rather than through [`Mesh::set_pos`].
    pub fn log_verts(&mut self, verts: &[u32]) {
        if self.log.is_none() {
            return;
        }
        for &v in verts {
            self.log_vert(v);
        }
    }

    #[inline]
    fn touch_vert(&mut self, v: u32) {
        if !self.fully_dirty {
            self.dirty_verts.push(v);
        }
    }

    #[inline]
    fn touch_face(&mut self, f: u32) {
        if !self.fully_dirty {
            self.dirty_faces.push(f);
        }
    }

    /// Hands over what changed and starts a fresh round.
    pub fn take_dirty(&mut self) -> (Vec<u32>, Vec<u32>, bool) {
        let full = self.fully_dirty;
        self.fully_dirty = false;
        (
            std::mem::take(&mut self.dirty_verts),
            std::mem::take(&mut self.dirty_faces),
            full,
        )
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

    /// Builds a grid only if there is none at all.
    ///
    /// What a stroke in progress wants. A rebuild walks the whole mesh, and
    /// between two dabs that is a freeze with the pen down; a grid whose cells
    /// no longer match the radius only costs a wider walk, and the stroke that
    /// follows will size it properly. The mismatch is bounded because the
    /// radius is settled before the stroke starts.
    pub fn ensure_accel_if_missing(&mut self, radius: f32) {
        if self.accel.is_none() {
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

    /// Refiles a vertex and notes the faces around it for later.
    ///
    /// The faces are not refiled here. A dab moves tens of thousands of
    /// vertices and a face has three of them, so doing it on the spot means
    /// working out the same triangle's centre three times, one thread at a
    /// time, in the middle of the stroke. Noting them and settling up at the
    /// end of the dab does each one once, and does the arithmetic on every
    /// core. Nothing reads a face cell in between.
    fn refile(&mut self, v: u32) {
        self.touch_vert(v);
        if self.accel.is_none() {
            return;
        }
        let p = self.verts[v as usize].pos;
        let moved = self.accel.as_mut().map(|g| g.move_vertex(v, p)).unwrap_or(false);
        if !moved {
            return;
        }
        self.stale_faces.extend_from_slice(&self.vfaces[v as usize]);
    }

    /// Puts every face noted since the last call back in the right cell.
    ///
    /// Cheap and safe to call when nothing is waiting, which is most of the
    /// time.
    pub fn flush_refit(&mut self) {
        if self.stale_faces.is_empty() || self.accel.is_none() {
            self.stale_faces.clear();
            return;
        }
        let mut stale = std::mem::take(&mut self.stale_faces);
        stale.sort_unstable();
        stale.dedup();
        let n = self.faces.len() as u32;
        stale.retain(|f| *f < n);
        // Where each face has ended up, worked out across the cores; the
        // filing itself is a scatter into one structure and stays here.
        let spheres: Vec<(Vec3, f32)> = stale
            .par_iter()
            .with_min_len(256)
            .map(|&f| self.face_sphere(f))
            .collect();
        if let Some(g) = &mut self.accel {
            for (&f, (c, r)) in stale.iter().zip(spheres) {
                g.move_face(f, c, r);
            }
        }
        stale.clear();
        self.stale_faces = stale;
    }

    /// Moves one vertex, keeping the spatial index correct.
    pub fn set_pos(&mut self, v: u32, p: Vec3) {
        self.log_vert(v);
        self.verts[v as usize].pos = p;
        self.refile(v);
    }

    /// Tells the index that these vertices were written to directly. Brushes
    /// batch their writes and call this once, which is much cheaper than going
    /// through [`Mesh::set_pos`] per vertex.
    pub fn commit_moves(&mut self, moved: &[u32]) {
        if !self.fully_dirty {
            self.dirty_verts.extend_from_slice(moved);
        }
        if self.accel.is_none() {
            return;
        }
        for &v in moved {
            self.refile(v);
        }
    }

    /// Records that these vertices were written without moving, which is what
    /// a colour or mask brush does.
    pub fn commit_attributes(&mut self, touched: &[u32]) {
        if !self.fully_dirty {
            self.dirty_verts.extend_from_slice(touched);
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
        self.touch_vert(id);
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
        self.touch_face(fi);
        fi
    }

    /// Removes a face, moving the last face into its slot.
    pub fn remove_face(&mut self, f: u32) {
        let fi = f as usize;
        // Two slots stop holding what they held: this one, which the last face
        // is about to move into, and the last one, which goes away.
        self.log_face(f);
        self.log_face((self.faces.len() - 1) as u32);
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
        // The last face now lives in this slot, so this slot's data changed.
        if fi < self.faces.len() {
            self.touch_face(f);
        }
    }

    /// Removes an isolated vertex, moving the last vertex into its slot.
    /// The caller must have removed every incident face first.
    pub fn remove_vertex(&mut self, v: u32) {
        let vi = v as usize;
        debug_assert!(self.vfaces[vi].is_empty(), "remove_vertex on a used vertex");
        let last = self.verts.len() - 1;
        self.log_vert(v);
        self.log_vert(last as u32);
        if vi != last {
            let moved: FaceList = self.vfaces[last].clone();
            for f in moved {
                self.log_face(f);
                for x in self.faces[f as usize].iter_mut() {
                    if *x == last as u32 {
                        *x = v;
                    }
                }
                // Those faces now name a different vertex index.
                self.touch_face(f);
            }
        }
        if let Some(g) = &mut self.accel {
            g.swap_remove_vertex(v);
        }
        self.verts.swap_remove(vi);
        self.vfaces.swap_remove(vi);
        if vi < self.verts.len() {
            self.touch_vert(v);
        }
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

    /// Rewrites one face in place, touching only the adjacency that changed.
    ///
    /// The alternative is removing the face and adding the new one, which walks
    /// the adjacency of six vertices, renumbers whatever `swap_remove` moved,
    /// and refiles two faces in the grid. Rewriting touches the one vertex that
    /// left and the one that arrived.
    fn replace_face(&mut self, f: u32, tri: [u32; 3]) {
        let old = self.faces[f as usize];
        if old == tri {
            return;
        }
        self.log_face(f);
        for &v in &old {
            if !tri.contains(&v) {
                self.vfaces[v as usize].retain(|x| *x != f);
            }
        }
        for &v in &tri {
            if !old.contains(&v) {
                self.vfaces[v as usize].push(f);
            }
        }
        self.faces[f as usize] = tri;
        if self.accel.is_some() {
            self.stale_faces.push(f);
        }
        self.touch_face(f);
    }

    /// Splits edge `(a, b)` at its midpoint, returning the new vertex.
    /// Winding of the affected faces is preserved.
    ///
    /// Each face across the edge becomes two, and the first of the two is the
    /// original face rewritten rather than a removal and two additions. A dab
    /// on a dense mesh does a couple of thousand of these, so what a single
    /// split costs is most of what dynamic topology costs.
    pub fn split_edge(&mut self, a: u32, b: u32) -> Option<u32> {
        let fs = self.faces_around_edge(a, b);
        if fs.is_empty() {
            return None;
        }
        let mid = Vertex::lerp_attrs(&self.verts[a as usize], &self.verts[b as usize], 0.5);
        let m = self.add_vertex(mid);

        // Face indices stay put: rewriting keeps them, and adding only appends.
        let pairs: SmallVec<[(u32, [u32; 3]); 2]> =
            fs.iter().map(|&f| (f, self.faces[f as usize])).collect();
        for (f, tri) in pairs {
            let i = tri.iter().position(|&x| x == a).unwrap();
            if tri[(i + 1) % 3] == b {
                // winding reads (a, b, o)
                let o = tri[(i + 2) % 3];
                self.replace_face(f, [a, m, o]);
                self.add_face([m, b, o]);
            } else {
                // winding reads (a, o, b)
                let o = tri[(i + 1) % 3];
                self.replace_face(f, [a, o, m]);
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
        self.log_vert(a);
        self.verts[a as usize] = merged;
        self.touch_vert(a);

        let mut doomed: SmallVec<[u32; 2]> = fs;
        doomed.sort_unstable_by(|x, y| y.cmp(x));
        for f in doomed {
            self.remove_face(f);
        }

        // Retarget b's remaining faces onto a.
        let rest: FaceList = self.vfaces[b as usize].clone();
        for f in rest {
            self.log_face(f);
            for x in self.faces[f as usize].iter_mut() {
                if *x == b {
                    *x = a;
                }
            }
            self.vfaces[a as usize].push(f);
            self.touch_face(f);
        }
        self.vfaces[b as usize].clear();
        self.remove_vertex(b);
        // `a` moved to the midpoint and inherited faces: refile everything it
        // now touches.
        self.refile(a);
        // Everything `a` now touches has a new centre, whether or not `a`
        // itself changed cell, since it inherited the faces of `b`.
        if self.accel.is_some() {
            let faces: FaceList = self.vfaces[a as usize].clone();
            self.stale_faces.extend_from_slice(&faces);
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
        self.verts
            .par_iter_mut()
            .zip(normals)
            .for_each(|(v, n)| v.nrm = n);
        self.mark_fully_dirty();
    }

    /// Recomputes normals for `verts` and their ring-1 neighbours only.
    ///
    /// Returns that wider set, which is exactly the set of vertices whose data
    /// changed and therefore what a renderer has to re-upload.
    ///
    /// The hot path of a dab on a dense mesh: a brush moves tens of thousands
    /// of vertices, and every one of them changes the normal of everything
    /// around it. Both halves run across every core, and both were written to
    /// avoid the work rather than to spread it.
    pub fn update_normals(&mut self, touched: &[u32]) -> Vec<u32> {
        if touched.is_empty() {
            return Vec::new();
        }
        // The ring around what moved. Duplicates are pushed and sorted out once
        // at the end: a neighbourhood that deduplicates as it goes tests every
        // candidate against everything it already holds, which is quadratic in
        // the valence and done once per vertex, where this is one sort.
        let (vfaces, faces) = (&self.vfaces, &self.faces);
        let ring = |v: u32| {
            vfaces[v as usize]
                .iter()
                .flat_map(|&f| faces[f as usize])
                .chain(std::iter::once(v))
        };
        let mut set: Vec<u32> = if touched.len() < PARALLEL_MIN {
            let mut s = Vec::with_capacity(touched.len() * 7);
            for &v in touched {
                s.extend(ring(v));
            }
            s
        } else {
            touched.par_iter().flat_map_iter(|&v| ring(v)).collect()
        };
        if set.len() < PARALLEL_MIN {
            set.sort_unstable();
        } else {
            set.par_sort_unstable();
        }
        set.dedup();

        // Collected, then written back. A parallel loop cannot write into the
        // vertices while another reads them, and the write is a cheap scatter
        // next to the cross products it carries.
        let verts = &self.verts;
        let normals: Vec<Vec3> = set
            .par_iter()
            .with_min_len(512)
            .map(|&v| {
                let mut n = Vec3::ZERO;
                for &f in &vfaces[v as usize] {
                    let [a, b, c] = faces[f as usize];
                    let pa = verts[a as usize].pos;
                    let pb = verts[b as usize].pos;
                    let pc = verts[c as usize].pos;
                    n += (pb - pa).cross(pc - pa);
                }
                n.normalize_or(Vec3::Y)
            })
            .collect();
        for (&v, n) in set.iter().zip(&normals) {
            self.verts[v as usize].nrm = *n;
        }

        if !self.fully_dirty {
            self.dirty_verts.extend_from_slice(&set);
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
        self.mark_fully_dirty();
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
        self.mark_fully_dirty();
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
