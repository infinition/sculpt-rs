//! Dynamic topology: local refinement and coarsening under the brush.
//!
//! This is what separates a sculpting tool from a mesh deformer. Detail is
//! created where the brush touches instead of being paid for up front over the
//! whole surface.

use crate::mesh::Mesh;
use crate::query;
use glam::Vec3;
use rayon::prelude::*;
use std::{cmp::Reverse, collections::BinaryHeap};

/// Vertices a thread takes at a time when classifying edges.
///
/// Large enough that handing out the work costs nothing next to doing it, small
/// enough that the last thread to finish is not holding up the other five.
const CHUNK: usize = 512;

/// What the detail setting is measured against.
///
/// A single target edge length in world units is the simplest thing to
/// implement and the worst to use. Zoom in on an ear and it keeps cutting to
/// the size that suited the whole head, so a close-up buries the machine under
/// vertices too small to see; zoom out and the brush stops adding anything at
/// all. The other two modes tie the target to something that follows the work.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DetailMode {
    /// A length in world units, the same everywhere on the model.
    ///
    /// What to use when the detail has to match across a piece regardless of
    /// how it was approached.
    #[default]
    Constant,
    /// A fraction of the brush radius.
    ///
    /// A small brush cuts fine, a large one stays coarse, which is how a hand
    /// works: you reach for a smaller tool to do smaller things.
    Radius,
    /// A length in pixels.
    ///
    /// The density is what you see, whatever the zoom. This is what stops a
    /// close-up from spending a million vertices on something that will cover
    /// a thumbnail, and it is the reason a dense session grinds to a halt when
    /// the only mode is a fixed world size.
    Screen,
}

impl DetailMode {
    pub const ALL: [DetailMode; 3] = [DetailMode::Constant, DetailMode::Radius, DetailMode::Screen];

    pub fn label(self) -> &'static str {
        match self {
            DetailMode::Constant => "World",
            DetailMode::Radius => "Brush",
            DetailMode::Screen => "Screen",
        }
    }

    /// What the detail number means in this mode, for an interface that has to
    /// print it next to a slider.
    pub fn unit(self) -> &'static str {
        match self {
            DetailMode::Constant => "",
            DetailMode::Radius => " x radius",
            DetailMode::Screen => " px",
        }
    }

    /// The range a slider should offer, and a sensible value inside it.
    ///
    /// The number means something different in each mode, so switching without
    /// resetting would read a length in world units as a count of pixels.
    pub fn range(self) -> (f32, f32) {
        match self {
            DetailMode::Constant => (0.002, 0.3),
            DetailMode::Radius => (0.02, 1.0),
            DetailMode::Screen => (2.0, 60.0),
        }
    }

    pub fn default_detail(self) -> f32 {
        match self {
            DetailMode::Constant => 0.04,
            DetailMode::Radius => 0.15,
            DetailMode::Screen => 8.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Dyntopo {
    /// Target edge length, read according to `mode`.
    pub detail: f32,
    /// What that number is measured in.
    pub mode: DetailMode,
    pub subdivide: bool,
    pub decimate: bool,
    /// Spread refinement over more dabs, keeping the requested edge length.
    pub responsive: bool,
    /// Safety valve: refuse to grow past this vertex count.
    pub max_verts: usize,
}

impl Default for Dyntopo {
    fn default() -> Self {
        Self {
            detail: 0.04,
            mode: DetailMode::Constant,
            subdivide: true,
            decimate: true,
            responsive: true,
            max_verts: 2_000_000,
        }
    }
}

impl Dyntopo {
    fn split_budget(&self) -> usize {
        if self.responsive { 1024 } else { MAX_SPLITS_PER_DAB }
    }
    /// The edge length a dab should aim for, in the object's own units.
    ///
    /// `radius` is the brush radius in those same units and `world_per_pixel`
    /// how much of them one pixel covers at the point being touched. A caller
    /// that cannot say passes zero, and the screen mode falls back rather than
    /// dividing by it.
    pub fn target_edge(&self, radius: f32, world_per_pixel: f32) -> f32 {
        let target = match self.mode {
            DetailMode::Constant => self.detail,
            DetailMode::Radius => radius * self.detail,
            DetailMode::Screen if world_per_pixel > 0.0 => self.detail * world_per_pixel,
            DetailMode::Screen => self.detail,
        };
        // A target of zero would ask for infinite subdivision, and the vertex
        // ceiling is a poor place to find that out.
        target.max(1e-5)
    }
}

/// Edges longer than this multiple of `detail` get split.
const SPLIT_FACTOR: f32 = 1.0;
/// Edges shorter than this multiple of `detail` get collapsed.
const COLLAPSE_FACTOR: f32 = 0.45;
/// Same split budget as the former three passes, maintained through a local
/// edge queue instead of rescanning the entire brush region between passes.
const MAX_SPLITS_PER_DAB: usize = 3 * 1024;
/// Most edges a pass will cut or close.
///
/// A dab on a dense mesh under a fine detail setting finds tens of thousands of
/// edges to cut, and cutting them all is what turns one dab into a hundred
/// milliseconds and a session into a stutter. The sorted plan names the worst
/// offenders first, so taking the head of the list cuts the worst of them and
/// leaves the rest for the next dab, which keeps a stroke responsive however
/// far it is from its target density. The mesh converges to the detail over
/// the stroke instead of inside a single dab.
const MAX_EDGES_PER_PASS: usize = 1024;

/// The edges a dab is about to cut or close, found before anything is touched.
///
/// Split out from the work so the caller can learn whether the topology is
/// about to move while there is still time to act on it: a stroke remembers
/// itself as moved vertices until the first cut, and once a cut has happened
/// there is no going back to record what was there before. Finding out by
/// looking twice would cost as much as the refinement itself.
#[derive(Default)]
pub struct Plan {
    /// Edges to cut, each with the length it had when it was found, longest
    /// first.
    long: Vec<(f32, (u32, u32))>,
    short: Vec<(u32, u32)>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.long.is_empty() && self.short.is_empty()
    }

    /// Ce que le plan a trouvé, pour les mesures de reproductibilité.
    pub fn edges(&self) -> (Vec<(u32, u32)>, Vec<(u32, u32)>) {
        (self.long.iter().map(|(_, e)| *e).collect(), self.short.clone())
    }

    /// Combien d'arêtes le plan touche, pour les mesures.
    pub fn len(&self) -> usize {
        self.long.len() + self.short.len()
    }
}

/// Classifies the edges around `verts` as too long or too short.
///
/// [`plan`] feeds it the whole brush sphere, and the later refinement passes
/// call it again through a fresh [`plan`] because splitting makes new edges.
///
/// Measuring an edge is two random reads into an array far too large for
/// cache, and every edge is independent of every other, so it is done a slice
/// of vertices at a time across the cores.
///
/// The slices come back in the order they went out, and both lists are then
/// put in a total order below, so what a plan says is settled by the mesh
/// and by nothing else. That is worth insisting on: the cutting follows this
/// list, and a list in a different order is a different mesh.
fn classify_edges(
    mesh: &Mesh,
    verts: &[u32],
    can_split: bool,
    decimate: bool,
    max_len: f32,
    min_len: f32,
    split_budget: usize,
) -> (Vec<(f32, (u32, u32))>, Vec<(u32, u32)>) {
    // Duplicates are left in instead of being hashed away. Taking an edge from
    // its lower-numbered vertex only cuts them from six to two, and the two are
    // harmless: a split revalidates through `faces_around_edge`, which is empty
    // the second time round, and a collapse revalidates its candidates anyway.
    // Two length comparisons are far cheaper than a hash table.
    let classify = |chunk: &[u32]| {
        let mut long: Vec<(f32, (u32, u32))> = Vec::new();
        let mut short: Vec<(u32, u32)> = Vec::new();
        for &v in chunk {
            for &f in &mesh.vfaces[v as usize] {
                let tri = mesh.faces[f as usize];
                for k in 0..3 {
                    let (a, b) = (tri[k], tri[(k + 1) % 3]);
                    // Each edge belongs to its lower-numbered end, and is only
                    // looked at while walking that end.
                    if a != v || a > b {
                        continue;
                    }
                    let len = mesh.edge_len(a, b);
                    if can_split && len > max_len {
                        long.push((len, (a, b)));
                    } else if decimate && len < min_len {
                        short.push((a, b));
                    }
                }
            }
        }
        (long, short)
    };

    let parts: Vec<(Vec<(f32, (u32, u32))>, Vec<(u32, u32)>)> = if verts.len() < CHUNK * 2 {
        vec![classify(verts)]
    } else {
        verts.par_chunks(CHUNK).map(classify).collect()
    };

    let mut long: Vec<(f32, (u32, u32))> =
        Vec::with_capacity(parts.iter().map(|(l, _)| l.len()).sum());
    let mut short: Vec<(u32, u32)> =
        Vec::with_capacity(parts.iter().map(|(_, s)| s.len()).sum());
    for (l, sh) in parts {
        long.extend_from_slice(&l);
        short.extend_from_slice(&sh);
    }
    // Longest first keeps refinement even instead of chasing one region.
    //
    // The length is carried out of the pass above rather than worked out again.
    // Sorting through a comparator that measures the edge does two distance
    // computations, each of them a pair of random reads into the vertex array,
    // for every one of the comparisons a sort makes: on the sixty thousand
    // candidates a dab on a dense mesh turns up, that is a couple of million of
    // them, and it dwarfed the cutting it was ordering. A length is never
    // negative, so its bits sort exactly as it does and the comparator goes
    // away entirely.
    // Two edges of the same length are ordered by their endpoints, and the short
    // list is ordered too, so neither depends on the order the slices happened
    // to turn things up in. Without this the refinement drifted by a handful of
    // edges from one run to the next on the same machine, which is invisible on
    // screen and intolerable in a test.
    //
    // It does not make a dab identical on machines with different core counts:
    // measured, a stroke lands on 7639 new vertices on six cores and 7649 on
    // one, each of them repeatable. Something else in a dab still follows how
    // the work was divided, and it is not this.
    // Only a bounded prefix of candidates can be used. Sorting every candidate
    // paid O(n log n) on high-poly patches. Selection
    // keeps the exact same prefix with O(n) work, then sorts that small prefix
    // to preserve the original deterministic application order.
    let key = |(len, e): &(f32, (u32, u32))| (std::cmp::Reverse(len.to_bits()), *e);
    if long.len() > split_budget {
        long.select_nth_unstable_by_key(split_budget, key);
        long.truncate(split_budget);
    }
    long.sort_unstable_by_key(key);
    if short.len() > MAX_EDGES_PER_PASS {
        short.select_nth_unstable(MAX_EDGES_PER_PASS);
        short.truncate(MAX_EDGES_PER_PASS);
    }
    short.sort_unstable();
    (long, short)
}

/// Works out what a dab here would change, without changing it.
///
/// One pass over the neighbourhood, classifying each edge as too long or too
/// short as it goes. It used to be two passes, each building a hash set of
/// sixty thousand edges to remove duplicates, which cost six milliseconds of
/// every dab on a dense mesh: more than the rest of the dab put together.
pub fn plan(mesh: &Mesh, center: Vec3, radius: f32, p: &Dyntopo) -> Plan {
    let can_split = p.subdivide && mesh.vert_count() < p.max_verts;
    if !can_split && !p.decimate {
        return Plan::default();
    }
    // Work slightly wider than the brush so the detail gradient is not sliced
    // off exactly at the falloff boundary.
    let max_len = p.detail * SPLIT_FACTOR;
    let min_len = p.detail * COLLAPSE_FACTOR;
    let verts = query::verts_in_sphere(mesh, center, radius * 1.15);
    let (long, short) = classify_edges(mesh, &verts, can_split, p.decimate, max_len, min_len, p.split_budget());
    Plan { long, short }
}

/// Refines the mesh inside the brush sphere. Returns true when topology changed.
pub fn refine(mesh: &mut Mesh, center: Vec3, radius: f32, p: &Dyntopo) -> bool {
    let plan = plan(mesh, center, radius, p);
    apply(mesh, plan, center, radius, p)
}

/// Carries out a plan. Only edges incident to a newly created midpoint need
/// classifying after a split; every untouched edge keeps its length.
pub fn apply(mesh: &mut Mesh, todo: Plan, center: Vec3, radius: f32, p: &Dyntopo) -> bool {
    let mut changed = false;

    if p.subdivide {
        let max_len = p.detail * SPLIT_FACTOR;
        let mut edges: BinaryHeap<_> = todo.long.into_iter()
            .map(|(len, edge)| (len.to_bits(), Reverse(edge)))
            .collect();
        let r2 = (radius * 1.15).powi(2);
        let mut splits = 0;
        while splits < p.split_budget() && mesh.vert_count() < p.max_verts {
            let Some((bits, Reverse((a, b)))) = edges.pop() else { break };
            if f32::from_bits(bits) <= max_len {
                break;
            }
            let Some(mid) = mesh.split_edge(a, b) else { continue };
            splits += 1;
            changed = true;
            // Splitting doesn't move existing vertices. The new edges are
            // exactly the midpoint's ring; every other queued length is valid.
            for v in mesh.neighbors(mid) {
                let edge = (v.min(mid), v.max(mid));
                // Match plan's brush-border rule (lower-index endpoint).
                if mesh.pos[edge.0 as usize].distance_squared(center) > r2 {
                    continue;
                }
                let len = mesh.edge_len(edge.0, edge.1);
                if len > max_len {
                    edges.push((len.to_bits(), Reverse(edge)));
                }
            }
        }
    }

    if p.decimate {
        let min_len = p.detail * COLLAPSE_FACTOR;
        // Collapses use swap_remove, so queued indices can go stale. Revalidate
        // each candidate instead of rebuilding the list.
        for (a, b) in todo.short {
            let n = mesh.vert_count() as u32;
            if a >= n || b >= n || a == b {
                continue;
            }
            if mesh.edge_len(a, b) >= min_len {
                continue;
            }
            if mesh.faces_around_edge(a, b).is_empty() {
                continue;
            }
            if mesh.collapse_edge(a, b) {
                changed = true;
            }
        }
    }

    changed
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_queue_refines_new_edges_to_the_target() {
        let mut mesh = crate::primitives::icosphere(1);
        mesh.ensure_accel(3.0);
        let params = Dyntopo { detail: 0.2, decimate: false, responsive: false, ..Default::default() };
        assert!(refine(&mut mesh, Vec3::ZERO, 3.0, &params));
        mesh.flush_refit();
        crate::tests::validate(&mesh);
        for &[a, b, c] in &mesh.faces {
            for (x, y) in [(a, b), (b, c), (c, a)] {
                assert!(mesh.edge_len(x, y) <= params.detail * 1.00001);
            }
        }
    }

    #[test]
    fn refinement_keeps_its_work_and_vertex_budgets() {
        for extra in [17, MAX_SPLITS_PER_DAB + 100] {
            let mut mesh = crate::primitives::icosphere(1);
            let before = mesh.vert_count();
            let params = Dyntopo {
                detail: 1e-4, decimate: false, responsive: false, max_verts: before + extra,
                ..Default::default()
            };
            refine(&mut mesh, Vec3::ZERO, 3.0, &params);
            assert_eq!(mesh.vert_count() - before, extra.min(MAX_SPLITS_PER_DAB));
            crate::tests::validate(&mesh);
        }
    }

    /// Chaque mode lit le même nombre dans son unité, et aucun ne peut rendre
    /// une longueur nulle: une cible à zéro demanderait une subdivision
    /// infinie, et le plafond de sommets est un mauvais endroit pour
    /// l'apprendre.
    #[test]
    fn each_mode_reads_the_detail_in_its_own_unit() {
        let radius = 0.2;
        let world_per_pixel = 0.001;

        let world = Dyntopo { detail: 0.05, mode: DetailMode::Constant, ..Default::default() };
        assert_eq!(world.target_edge(radius, world_per_pixel), 0.05);

        let brush = Dyntopo { detail: 0.25, mode: DetailMode::Radius, ..Default::default() };
        assert!((brush.target_edge(radius, world_per_pixel) - 0.05).abs() < 1e-6);

        let screen = Dyntopo { detail: 8.0, mode: DetailMode::Screen, ..Default::default() };
        assert!((screen.target_edge(radius, world_per_pixel) - 0.008).abs() < 1e-6);

        for mode in DetailMode::ALL {
            let d = Dyntopo { detail: 0.0, mode, ..Default::default() };
            assert!(d.target_edge(radius, world_per_pixel) > 0.0, "{mode:?} rend zéro");
        }
    }

    /// Le mode écran doit se rabattre quand l'appelant ne sait pas dire ce que
    /// vaut un pixel, plutôt que de rendre une cible absurde.
    #[test]
    fn the_screen_mode_falls_back_when_the_scale_is_unknown() {
        let d = Dyntopo { detail: 8.0, mode: DetailMode::Screen, ..Default::default() };
        assert_eq!(d.target_edge(0.2, 0.0), 8.0);
    }

    /// Zoomer doit resserrer le détail à l'écran, pas dans le monde: c'est
    /// toute la raison du mode.
    #[test]
    fn the_screen_mode_follows_the_zoom() {
        let d = Dyntopo { detail: 8.0, mode: DetailMode::Screen, ..Default::default() };
        let loin = d.target_edge(0.2, 0.01);
        let près = d.target_edge(0.2, 0.001);
        assert!(près < loin, "de près {près} n'est pas plus fin que de loin {loin}");
    }
}
