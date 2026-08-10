//! Dynamic topology: local refinement and coarsening under the brush.
//!
//! This is what separates a sculpting tool from a mesh deformer. Detail is
//! created where the brush touches instead of being paid for up front over the
//! whole surface.

use crate::mesh::Mesh;
use crate::query;
use glam::Vec3;
use rustc_hash::FxHashSet;

#[derive(Clone, Copy, Debug)]
pub struct Dyntopo {
    /// Target edge length under the brush, in world units.
    pub detail: f32,
    pub subdivide: bool,
    pub decimate: bool,
    /// Safety valve: refuse to grow past this vertex count.
    pub max_verts: usize,
}

impl Default for Dyntopo {
    fn default() -> Self {
        Self { detail: 0.04, subdivide: true, decimate: true, max_verts: 2_000_000 }
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
    long: Vec<(u32, u32)>,
    short: Vec<(u32, u32)>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.long.is_empty() && self.short.is_empty()
    }
}

/// Works out what a dab here would change, without changing it.
pub fn plan(mesh: &Mesh, center: Vec3, radius: f32, p: &Dyntopo) -> Plan {
    let r = radius * 1.15;
    let long = if p.subdivide && mesh.vert_count() < p.max_verts {
        collect_edges(mesh, center, r, |len| len > p.detail * SPLIT_FACTOR)
    } else {
        Vec::new()
    };
    let short = if p.decimate {
        collect_edges(mesh, center, r, |len| len < p.detail * COLLAPSE_FACTOR)
    } else {
        Vec::new()
    };
    Plan { long, short }
}

/// Refines the mesh inside the brush sphere. Returns true when topology changed.
pub fn refine(mesh: &mut Mesh, center: Vec3, radius: f32, p: &Dyntopo) -> bool {
    let plan = plan(mesh, center, radius, p);
    apply(mesh, plan, center, radius, p)
}

/// Carries out a plan. The first pass reuses what the plan already found; the
/// later ones have to look again, since splitting makes new edges.
pub fn apply(mesh: &mut Mesh, plan: Plan, center: Vec3, radius: f32, p: &Dyntopo) -> bool {
    let mut changed = false;
    // Work slightly wider than the brush so the detail gradient is not sliced
    // off exactly at the falloff boundary.
    let r = radius * 1.15;
    let mut first_long = Some(plan.long);

    if p.subdivide {
        let max_len = p.detail * SPLIT_FACTOR;
        for _ in 0..PASSES {
            if mesh.vert_count() >= p.max_verts {
                break;
            }
            let long = match first_long.take() {
                Some(found) => found,
                None => collect_edges(mesh, center, r, |len| len > max_len),
            };
            if long.is_empty() {
                break;
            }
            // Vertex indices stay stable across splits (vertices are only
            // appended), so the whole batch can be applied in one go.
            for (a, b) in long {
                if mesh.vert_count() >= p.max_verts {
                    break;
                }
                if mesh.edge_len(a, b) > max_len {
                    mesh.split_edge(a, b);
                    changed = true;
                }
            }
        }
    }

    if p.decimate {
        let min_len = p.detail * COLLAPSE_FACTOR;
        // Collapses use swap_remove, so queued indices can go stale. Revalidate
        // each candidate instead of rebuilding the list.
        for (a, b) in plan.short {
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

/// Unique edges of the faces inside the sphere that satisfy `pred(length)`.
fn collect_edges<F>(mesh: &Mesh, center: Vec3, radius: f32, pred: F) -> Vec<(u32, u32)>
where
    F: Fn(f32) -> bool,
{
    let faces = query::faces_in_sphere(mesh, center, radius);
    let mut seen: FxHashSet<(u32, u32)> = FxHashSet::default();
    let mut out = Vec::new();
    for f in faces {
        let tri = mesh.faces[f as usize];
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            let key = if a < b { (a, b) } else { (b, a) };
            if !seen.insert(key) {
                continue;
            }
            if pred(mesh.edge_len(a, b)) {
                out.push(key);
            }
        }
    }
    // Longest first keeps refinement even instead of chasing one region.
    out.sort_unstable_by(|x, y| {
        mesh.edge_len(y.0, y.1)
            .partial_cmp(&mesh.edge_len(x.0, x.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}
