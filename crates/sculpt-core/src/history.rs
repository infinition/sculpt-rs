//! Undo/redo.
//!
//! Full snapshots, bounded by a memory budget. Dynamic topology rewrites both
//! arrays wholesale, so a delta scheme would degenerate to a full copy on most
//! strokes anyway. Adjacency is not stored; it is cheaper to rebuild on restore
//! than to keep a second copy of it around.

use crate::mesh::{Mesh, Vertex};
use std::collections::VecDeque;

pub struct Snapshot {
    verts: Vec<Vertex>,
    faces: Vec<[u32; 3]>,
}

impl Snapshot {
    fn of(mesh: &Mesh) -> Self {
        Self { verts: mesh.verts.clone(), faces: mesh.faces.clone() }
    }

    fn bytes(&self) -> usize {
        self.verts.len() * std::mem::size_of::<Vertex>()
            + self.faces.len() * std::mem::size_of::<[u32; 3]>()
    }

    fn restore_into(self, mesh: &mut Mesh) {
        mesh.verts = self.verts;
        mesh.faces = self.faces;
        mesh.rebuild_adjacency();
    }
}

pub struct History {
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
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

    /// Records the state before a stroke begins.
    pub fn push(&mut self, mesh: &Mesh) {
        let snap = Snapshot::of(mesh);
        self.used += snap.bytes();
        self.undo.push_back(snap);
        self.redo.clear();
        while self.used > self.budget && self.undo.len() > 1 {
            if let Some(old) = self.undo.pop_front() {
                self.used -= old.bytes();
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self, mesh: &mut Mesh) -> bool {
        let Some(prev) = self.undo.pop_back() else {
            return false;
        };
        self.used -= prev.bytes();
        self.redo.push(Snapshot::of(mesh));
        prev.restore_into(mesh);
        true
    }

    pub fn redo(&mut self, mesh: &mut Mesh) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push_back(Snapshot::of(mesh));
        self.used += self.undo.back().map(|s| s.bytes()).unwrap_or(0);
        next.restore_into(mesh);
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
}
