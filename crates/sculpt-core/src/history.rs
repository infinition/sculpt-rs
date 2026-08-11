//! Undo/redo.
//!
//! Two kinds of step. A stroke or a topology command snapshots the geometry of
//! the object it touched; a structural change (adding, deleting or merging
//! objects) snapshots the whole scene, which is rare enough that the extra copy
//! does not matter. Adjacency and the spatial index are not stored: rebuilding
//! them on restore is cheaper than keeping a second copy alive.

use crate::mesh::{Mesh, TopoLog, Vertex};
use crate::scene::{Object, Scene, Transform};

use std::collections::VecDeque;

pub enum Step {
    /// Geometry of one object.
    Geometry {
        index: usize,
        verts: Vec<Vertex>,
        faces: Vec<[u32; 3]>,
    },
    /// Only the slots a stroke wrote over, as they were before it did, and the
    /// lengths the arrays had when it started.
    ///
    /// A stroke moves a few thousand vertices out of several million. Keeping a
    /// copy of the whole model for that was costing forty milliseconds and two
    /// hundred megabytes per stroke on a five million triangle mesh, which is
    /// most of what made a dense session stutter and then run out of memory.
    ///
    /// Unlike the list of moved vertices it replaces, this survives dynamic
    /// topology: an array is its length and its contents, so the original value
    /// of every slot that was written or dropped is enough to rebuild it, and a
    /// stroke that cuts edges is no different from one that does not.
    Topology {
        index: usize,
        vlen: usize,
        flen: usize,
        verts: Vec<(u32, Vertex)>,
        faces: Vec<(u32, [u32; 3])>,
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
            verts: (0..o.mesh.vert_count() as u32).map(|v| o.mesh.vertex(v)).collect(),
            faces: o.mesh.faces.clone(),
        })
    }

    fn bytes(&self) -> usize {
        match self {
            Step::Geometry { verts, faces, .. } => {
                verts.len() * std::mem::size_of::<Vertex>()
                    + faces.len() * std::mem::size_of::<[u32; 3]>()
            }
            Step::Topology { verts, faces, .. } => {
                verts.len() * (std::mem::size_of::<Vertex>() + 8)
                    + faces.len() * (std::mem::size_of::<[u32; 3]>() + 8)
            }
            Step::Placement { .. } => std::mem::size_of::<Transform>(),
            Step::Scene { objects, .. } => objects
                .iter()
                .map(|o| {
                    o.mesh.pos.len() * std::mem::size_of::<Vertex>()
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
                    o.mesh.resize_verts(verts.len());
                    for (i, v) in verts.iter().enumerate() {
                        o.mesh.write_vertex(i as u32, v);
                    }
                    o.mesh.faces = faces;
                    o.mesh.rebuild_adjacency();
                }
                current.unwrap_or(Step::Placement { index, transform: Transform::default() })
            }
            Step::Topology { index, vlen, flen, verts, faces } => {
                let Some(o) = scene.objects.get_mut(index) else {
                    return Step::Placement { index, transform: Transform::default() };
                };
                let m = &mut o.mesh;
                let (now_vlen, now_flen) = (m.pos.len(), m.faces.len());

                // What putting this back is about to overwrite is exactly what
                // the opposite step will have to put back in turn: the slots
                // named here, plus whatever falls off the end when the arrays
                // go back to their old length.
                let now_verts = capture(&verts, vlen, now_vlen, |i| m.vertex(i as u32));
                let now_faces = capture(&faces, flen, now_flen, |i| m.faces[i]);

                // A stroke that only moved vertices leaves the arrays the shape
                // they were, and does not need the adjacency rebuilt: on a
                // dense mesh that is the difference between an undo that lands
                // at once and one that stops for a tenth of a second.
                let structural = !faces.is_empty() || vlen != now_vlen || flen != now_flen;

                m.resize_verts(vlen);
                m.faces.resize(flen, [0, 0, 0]);
                for (i, v) in &verts {
                    m.write_vertex(*i, v);
                }
                for (i, t) in &faces {
                    m.faces[*i as usize] = *t;
                }

                let touched: Vec<u32> = verts
                    .iter()
                    .map(|(v, _)| *v)
                    .filter(|v| (*v as usize) < m.pos.len())
                    .collect();
                if structural {
                    m.rebuild_adjacency();
                }
                // The ring around a restored vertex has a normal that came from
                // where it used to be, whether or not the ring itself was
                // recorded.
                m.update_normals(&touched);

                Step::Topology {
                    index,
                    vlen: now_vlen,
                    flen: now_flen,
                    verts: now_verts,
                    faces: now_faces,
                }
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

/// The slots an undo is about to write over, kept so it can be redone.
///
/// Two sets of them: the ones the step names, and the tail that goes away when
/// the array is cut back to the length it had before. Anything the step names
/// beyond the current end has already gone and needs no copy, since redoing
/// will cut back to here anyway.
fn capture<T: Copy>(
    named: &[(u32, T)],
    old_len: usize,
    now_len: usize,
    read: impl Fn(usize) -> T,
) -> Vec<(u32, T)> {
    let mut keys: Vec<u32> = named.iter().map(|(i, _)| *i).collect();
    keys.extend(old_len as u32..now_len as u32);
    keys.sort_unstable();
    keys.dedup();
    keys.retain(|i| (*i as usize) < now_len);
    keys.into_iter().map(|i| (i, read(i as usize))).collect()
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

    /// Records what a stroke wrote over, from the log the mesh kept while it
    /// was happening.
    pub fn push_topology(&mut self, index: usize, log: TopoLog) {
        if log.is_empty() {
            return;
        }
        self.push(Step::Topology {
            index,
            vlen: log.vlen,
            flen: log.flen,
            verts: log.verts,
            faces: log.faces,
        });
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
    (
        (0..mesh.vert_count() as u32).map(|v| mesh.vertex(v)).collect(),
        mesh.faces.clone(),
    )
}
