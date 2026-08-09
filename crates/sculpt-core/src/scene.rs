//! A scene of independently transformed meshes.
//!
//! Sculpting always happens in the active object's local space, so the app
//! pushes rays through [`Transform::inverse_matrix`] before picking and pulls
//! the resulting stroke back out. Keeping the transform out of the vertex data
//! means moving an object never dirties its GPU buffers.

use crate::mesh::Mesh;
use glam::{Mat4, Quat, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }
}

impl Transform {
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }

    pub fn inverse_matrix(&self) -> Mat4 {
        self.matrix().inverse()
    }

    /// Uniform scale factor, used to convert a screen-space brush radius into
    /// object space.
    pub fn mean_scale(&self) -> f32 {
        (self.scale.x.abs() + self.scale.y.abs() + self.scale.z.abs()) / 3.0
    }

    pub fn is_identity(&self) -> bool {
        *self == Transform::default()
    }
}

#[derive(Clone)]
pub struct Object {
    pub name: String,
    pub mesh: Mesh,
    pub transform: Transform,
    pub visible: bool,
}

impl Object {
    pub fn new(name: impl Into<String>, mesh: Mesh) -> Self {
        Self { name: name.into(), mesh, transform: Transform::default(), visible: true }
    }

    /// World-space bounds of the object's local bounding box.
    pub fn world_bounds(&self) -> (Vec3, Vec3) {
        let (lo, hi) = self.mesh.bounds();
        if self.transform.is_identity() {
            return (lo, hi);
        }
        let m = self.transform.matrix();
        let mut wlo = Vec3::splat(f32::MAX);
        let mut whi = Vec3::splat(f32::MIN);
        for k in 0..8 {
            let c = Vec3::new(
                if k & 1 == 0 { lo.x } else { hi.x },
                if k & 2 == 0 { lo.y } else { hi.y },
                if k & 4 == 0 { lo.z } else { hi.z },
            );
            let p = m.transform_point3(c);
            wlo = wlo.min(p);
            whi = whi.max(p);
        }
        (wlo, whi)
    }

    /// Bakes the transform into the vertices and resets it.
    pub fn apply_transform(&mut self) {
        if self.transform.is_identity() {
            return;
        }
        let m = self.transform.matrix();
        self.mesh.apply_transform(m);
        self.transform = Transform::default();
    }
}

#[derive(Clone)]
pub struct Scene {
    pub objects: Vec<Object>,
    pub active: usize,
}

impl Default for Scene {
    fn default() -> Self {
        Self { objects: Vec::new(), active: 0 }
    }
}

impl Scene {
    pub fn with_object(obj: Object) -> Self {
        Self { objects: vec![obj], active: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn active(&self) -> Option<&Object> {
        self.objects.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut Object> {
        self.objects.get_mut(self.active)
    }

    pub fn add(&mut self, obj: Object) -> usize {
        self.objects.push(obj);
        self.active = self.objects.len() - 1;
        self.active
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.objects.len() {
            return;
        }
        self.objects.remove(index);
        self.active = self.active.min(self.objects.len().saturating_sub(1));
    }

    pub fn duplicate(&mut self, index: usize) -> Option<usize> {
        let src = self.objects.get(index)?.clone();
        let mut copy = src;
        copy.name = format!("{} copy", copy.name);
        Some(self.add(copy))
    }

    /// Merges every visible object into one, baking transforms as it goes.
    pub fn merge_visible(&mut self) -> Option<usize> {
        let visible: Vec<usize> = self
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.visible)
            .map(|(i, _)| i)
            .collect();
        if visible.len() < 2 {
            return None;
        }
        let mut merged = Mesh::new();
        for &i in &visible {
            let mut o = self.objects[i].clone();
            o.apply_transform();
            let base = merged.verts.len() as u32;
            merged.verts.extend_from_slice(&o.mesh.verts);
            merged
                .faces
                .extend(o.mesh.faces.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
        }
        merged.rebuild_adjacency();
        merged.recompute_normals();

        for &i in visible.iter().rev() {
            self.objects.remove(i);
        }
        Some(self.add(Object::new("merged", merged)))
    }

    /// Bounds covering everything visible.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        let mut any = false;
        for o in self.objects.iter().filter(|o| o.visible && o.mesh.vert_count() > 0) {
            let (a, b) = o.world_bounds();
            lo = lo.min(a);
            hi = hi.max(b);
            any = true;
        }
        if any { (lo, hi) } else { (Vec3::ZERO, Vec3::ZERO) }
    }

    pub fn total_verts(&self) -> usize {
        self.objects.iter().map(|o| o.mesh.vert_count()).sum()
    }

    pub fn total_faces(&self) -> usize {
        self.objects.iter().map(|o| o.mesh.face_count()).sum()
    }
}
