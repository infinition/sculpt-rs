//! Undo/redo.
//!
//! Two kinds of step. A stroke or a topology command snapshots the geometry of
//! the object it touched; a structural change (adding, deleting or merging
//! objects) snapshots the whole scene, which is rare enough that the extra copy
//! does not matter. Adjacency and the spatial index are not stored: rebuilding
//! them on restore is cheaper than keeping a second copy alive.

use crate::mesh::{Mesh, Vertex};
use crate::scene::{Object, Scene, Transform};
use std::collections::VecDeque;

pub enum Step {
    /// Geometry of one object.
    Geometry {
        index: usize,
        verts: Vec<Vertex>,
        faces: Vec<[u32; 3]>,
    },
    /// Only the vertices a stroke touched, as they were before it did.
    ///
    /// A stroke moves a few thousand vertices out of several million. Keeping a
    /// copy of the whole model for that was costing forty milliseconds and two
    /// hundred megabytes per stroke on a five million triangle mesh, which is
    /// most of what made a dense session stutter and then run out of memory.
    /// Only valid while the topology holds still, since the indices are the
    /// whole of the reference.
    VertexDelta {
        index: usize,
        before: Vec<(u32, Vertex)>,
    },
    /// Placement of one object, for gizmo moves.
    Placement { index: usize, transform: Transform },
    /// Everything, for structural edits.
    Scene { objects: Vec<Object>, active: usize },
}

impl Step {
    fn geometry_of(scene: &Scene, index: usize) -> Option<Step> {
        let o = scene.objects.get(index)?;
        Some(Step::Geometry {
            index,
            verts: o.mesh.verts.clone(),
            faces: o.mesh.faces.clone(),
        })
    }

    fn bytes(&self) -> usize {
        match self {
            Step::Geometry { verts, faces, .. } => {
                verts.len() * std::mem::size_of::<Vertex>()
                    + faces.len() * std::mem::size_of::<[u32; 3]>()
            }
            Step::VertexDelta { before, .. } => {
                before.len() * (std::mem::size_of::<Vertex>() + 4)
            }
            Step::Placement { .. } => std::mem::size_of::<Transform>(),
            Step::Scene { objects, .. } => objects
                .iter()
                .map(|o| {
                    o.mesh.verts.len() * std::mem::size_of::<Vertex>()
                        + o.mesh.faces.len() * std::mem::size_of::<[u32; 3]>()
                })
                .sum(),
        }
    }

    /// Swaps this snapshot with the live scene, so the caller can push the
    /// result onto the opposite stack.
    fn swap(self, scene: &mut Scene) -> Step {
        match self {
            Step::Geometry { index, verts, faces } => {
                let current = Step::geometry_of(scene, index);
                if let Some(o) = scene.objects.get_mut(index) {
                    o.mesh.verts = verts;
                    o.mesh.faces = faces;
                    o.mesh.rebuild_adjacency();
                }
                current.unwrap_or(Step::Placement { index, transform: Transform::default() })
            }
            Step::VertexDelta { index, before } => {
                let mut now = Vec::with_capacity(before.len());
                if let Some(o) = scene.objects.get_mut(index) {
                    // An index that no longer exists is dropped rather than
                    // trusted. It can only happen if the topology moved under a
                    // delta, which is exactly what this step promises it did
                    // not, but a corrupt mesh is a worse answer than a lost
                    // vertex.
                    for (v, old) in &before {
                        let i = *v as usize;
                        if i < o.mesh.verts.len() {
                            now.push((*v, o.mesh.verts[i]));
                            o.mesh.verts[i] = *old;
                        }
                    }
                    let touched: Vec<u32> = now.iter().map(|(v, _)| *v).collect();
                    // Positions changed, so the normals around them did too.
                    o.mesh.update_normals(&touched);
                }
                Step::VertexDelta { index, before: now }
            }
            Step::Placement { index, transform } => {
                let current = scene
                    .objects
                    .get(index)
                    .map(|o| o.transform)
                    .unwrap_or_default();
                if let Some(o) = scene.objects.get_mut(index) {
                    o.transform = transform;
                }
                Step::Placement { index, transform: current }
            }
            Step::Scene { objects, active } => {
                let current = Step::Scene {
                    objects: scene.objects.clone(),
                    active: scene.active,
                };
                scene.objects = objects;
                scene.active = active.min(scene.objects.len().saturating_sub(1));
                current
            }
        }
    }
}

/// What a stroke has already taken a copy of.
///
/// A dab writes the same vertices over and over as the hand moves back across
/// its own path, so the first write is the only one worth keeping. The set is
/// what makes that cheap.
#[derive(Default)]
pub struct StrokeJournal {
    seen: rustc_hash::FxHashSet<u32>,
    before: Vec<(u32, Vertex)>,
}

impl StrokeJournal {
    /// Copies these vertices as they are now, skipping any already copied.
    ///
    /// Must be called before the dab writes, which is the whole contract.
    pub fn record(&mut self, verts: &[u32], mesh: &Mesh) {
        self.before.reserve(verts.len());
        for &v in verts {
            if self.seen.insert(v) {
                if let Some(vx) = mesh.verts.get(v as usize) {
                    self.before.push((v, *vx));
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.before.is_empty()
    }

    pub fn into_before(self) -> Vec<(u32, Vertex)> {
        self.before
    }
}

pub struct History {
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    budget: usize,
    used: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(768 * 1024 * 1024)
    }
}

impl History {
    pub fn new(budget_bytes: usize) -> Self {
        Self { undo: VecDeque::new(), redo: Vec::new(), budget: budget_bytes, used: 0 }
    }

    fn push(&mut self, step: Step) {
        self.used += step.bytes();
        self.undo.push_back(step);
        self.redo.clear();
        while self.used > self.budget && self.undo.len() > 1 {
            if let Some(old) = self.undo.pop_front() {
                self.used -= old.bytes();
            }
        }
    }

    /// Records the geometry of one object before it is edited.
    pub fn push_geometry(&mut self, scene: &Scene, index: usize) {
        if let Some(step) = Step::geometry_of(scene, index) {
            self.push(step);
        }
    }

    /// Records only the vertices a stroke touched.
    pub fn push_vertex_delta(&mut self, index: usize, before: Vec<(u32, Vertex)>) {
        if before.is_empty() {
            return;
        }
        self.push(Step::VertexDelta { index, before });
    }

    /// Throws away everything, and says how much that freed.
    ///
    /// Used when an operation needs the room more than the history does. Undo
    /// is a convenience; failing to allocate is the end of the session.
    pub fn release(&mut self) -> usize {
        let freed = self.used;
        self.clear();
        freed
    }

    /// Keeps the budget in proportion to what is being edited.
    ///
    /// A fixed budget is wrong at both ends: far too much for a small model,
    /// and still only two strokes deep on a large one while holding a gigabyte.
    /// Six times the geometry, floored and capped, keeps a useful depth without
    /// competing with the mesh for memory.
    pub fn fit_budget_to(&mut self, mesh_bytes: usize) {
        const FLOOR: usize = 128 * 1024 * 1024;
        const CEILING: usize = 1024 * 1024 * 1024;
        self.budget = (mesh_bytes.saturating_mul(6)).clamp(FLOOR, CEILING);
        while self.used > self.budget && self.undo.len() > 1 {
            if let Some(old) = self.undo.pop_front() {
                self.used -= old.bytes();
            }
        }
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget
    }

    pub fn push_placement(&mut self, scene: &Scene, index: usize) {
        if let Some(o) = scene.objects.get(index) {
            self.push(Step::Placement { index, transform: o.transform });
        }
    }

    /// Records the whole scene before a structural change.
    pub fn push_scene(&mut self, scene: &Scene) {
        self.push(Step::Scene {
            objects: scene.objects.clone(),
            active: scene.active,
        });
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self, scene: &mut Scene) -> bool {
        let Some(prev) = self.undo.pop_back() else {
            return false;
        };
        self.used -= prev.bytes();
        let current = prev.swap(scene);
        self.redo.push(current);
        true
    }

    pub fn redo(&mut self, scene: &mut Scene) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let current = next.swap(scene);
        self.used += current.bytes();
        self.undo.push_back(current);
        true
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.used = 0;
    }

    pub fn used_bytes(&self) -> usize {
        self.used
    }

    pub fn depth(&self) -> usize {
        self.undo.len()
    }
}

/// Convenience for tests and callers that only ever hold one mesh.
pub fn snapshot_mesh(mesh: &Mesh) -> (Vec<Vertex>, Vec<[u32; 3]>) {
    (mesh.verts.clone(), mesh.faces.clone())
}
