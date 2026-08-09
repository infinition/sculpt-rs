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
