//! Dynamic topology: local refinement and coarsening under the brush.
//!
//! This is what separates a sculpting tool from a mesh deformer. Detail is
//! created where the brush touches instead of being paid for up front over the
//! whole surface.

use crate::mesh::Mesh;
use crate::query;
use glam::Vec3;
use rayon::prelude::*;

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
            max_verts: 2_000_000,
        }
    }
}

impl Dyntopo {
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
/// Refinement passes per stroke step.
const PASSES: usize = 3;

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
    long.sort_unstable_by_key(|(len, e)| (std::cmp::Reverse(len.to_bits()), *e));
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
    let (long, short) = classify_edges(mesh, &verts, can_split, p.decimate, max_len, min_len);
    Plan { long, short }
}

/// Refines the mesh inside the brush sphere. Returns true when topology changed.
pub fn refine(mesh: &mut Mesh, center: Vec3, radius: f32, p: &Dyntopo) -> bool {
    let plan = plan(mesh, center, radius, p);
    apply(mesh, plan, center, radius, p)
}

/// Carries out a plan. The first pass reuses what the plan already found; the
/// later ones have to look again, since splitting makes new edges.
pub fn apply(mesh: &mut Mesh, todo: Plan, center: Vec3, radius: f32, p: &Dyntopo) -> bool {
    let mut changed = false;
    let mut first_long = Some(todo.long);

    if p.subdivide {
        let max_len = p.detail * SPLIT_FACTOR;
        for _ in 0..PASSES {
            if mesh.vert_count() >= p.max_verts {
                break;
            }
            let long = match first_long.take() {
                Some(found) => found,
                None => {
                    // Splitting makes new edges, so the later passes have to
                    // look again. Only the splitting half is wanted here.
                    let again = Dyntopo { decimate: false, ..*p };
                    plan(mesh, center, radius, &again).long
                }
            };
            if long.is_empty() {
                break;
            }
            // The length was measured when the plan was made, and a split never
            // moves its two endpoints, so it is still true when the edge is
            // reached. Re-measuring would be two random reads per candidate for
            // the same answer.
            let mut split_any = false;
            for (len, (a, b)) in long {
                if mesh.vert_count() >= p.max_verts {
                    break;
                }
                if len > max_len {
                    if mesh.split_edge(a, b).is_some() {
                        split_any = true;
                        changed = true;
                    }
                }
            }
            // Splitting is what makes the new edges a later pass would find. If
            // a pass cut nothing, the region is at its target density and the
            // next pass would replan and cut nothing again, so it is skipped
            // rather than paying for a classification it cannot use.
            if !split_any {
                break;
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
