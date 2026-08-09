//! Core sculpting engine: mesh, dynamic topology, brushes, history, I/O.
//!
//! Free of any graphics dependency so it can be driven by the desktop app, a
//! headless test, or a future wasm build.

pub mod brush;
pub mod dyntopo;
pub mod history;
pub mod io;
pub mod mesh;
pub mod primitives;
pub mod query;

pub use brush::{Brush, BrushKind, StrokeInput};
pub use dyntopo::Dyntopo;
pub use history::History;
pub use mesh::{Mesh, Vertex};
pub use query::Hit;

use glam::Vec3;

/// Drives a mesh through strokes, keeping topology, normals and undo in sync.
pub struct Sculptor {
    pub mesh: Mesh,
    pub brush: Brush,
    pub dyntopo: Dyntopo,
    pub dyntopo_enabled: bool,
    pub symmetry_x: bool,
    pub history: History,
    /// Set when vertex data changed; the renderer clears it after upload.
    pub verts_dirty: bool,
    /// Set when the index buffer needs a re-upload.
    pub topology_dirty: bool,
    stroking: bool,
}

impl Sculptor {
    pub fn new(mesh: Mesh) -> Self {
        let mut s = Self {
            brush: Brush::default(),
            dyntopo: Dyntopo::default(),
            dyntopo_enabled: true,
            symmetry_x: true,
            history: History::default(),
            verts_dirty: true,
            topology_dirty: true,
            stroking: false,
            mesh,
        };
        s.reset_detail_to_mesh();
        s
    }

    /// Seeds the dyntopo target from the current tessellation so a freshly
    /// loaded mesh does not immediately explode or collapse under the brush.
    pub fn reset_detail_to_mesh(&mut self) {
        let e = self.mesh.mean_edge_len();
        if e.is_finite() && e > 0.0 {
            self.dyntopo.detail = e;
        }
    }

    pub fn replace_mesh(&mut self, mesh: Mesh) {
        self.mesh = mesh;
        self.history.clear();
        self.reset_detail_to_mesh();
        self.verts_dirty = true;
        self.topology_dirty = true;
    }

    pub fn begin_stroke(&mut self) {
        if !self.stroking {
            self.history.push(&self.mesh);
            self.stroking = true;
        }
    }

    pub fn end_stroke(&mut self) {
        self.stroking = false;
    }

    /// One dab, including its mirrored twin when symmetry is on.
    pub fn stroke(&mut self, input: &StrokeInput) {
        self.dab(input);
        if self.symmetry_x {
            let m = input.mirrored_x();
            self.dab(&m);
        }
    }

    fn dab(&mut self, input: &StrokeInput) {
        let deforms = self.brush.kind.deforms();

        if deforms && self.dyntopo_enabled {
            let changed = dyntopo::refine(
                &mut self.mesh,
                input.point,
                self.brush.radius,
                &self.dyntopo,
            );
            if changed {
                self.topology_dirty = true;
            }
        }

        let touched = brush::apply(&mut self.mesh, &self.brush, input);
        if !touched.is_empty() {
            if deforms {
                self.mesh.update_normals(&touched);
            }
            self.verts_dirty = true;
        }
    }

    pub fn undo(&mut self) {
        if self.history.undo(&mut self.mesh) {
            self.verts_dirty = true;
            self.topology_dirty = true;
        }
    }

    pub fn redo(&mut self) {
        if self.history.redo(&mut self.mesh) {
            self.verts_dirty = true;
            self.topology_dirty = true;
        }
    }

    pub fn clear_mask(&mut self) {
        for v in self.mesh.verts.iter_mut() {
            v.mask = 0.0;
        }
        self.verts_dirty = true;
    }

    pub fn invert_mask(&mut self) {
        for v in self.mesh.verts.iter_mut() {
            v.mask = 1.0 - v.mask;
        }
        self.verts_dirty = true;
    }

    /// Casts a ray against the surface.
    pub fn pick(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        query::raycast(&self.mesh, origin, dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks that adjacency agrees with the face array in both directions.
    fn validate(m: &Mesh) {
        assert_eq!(m.verts.len(), m.vfaces.len(), "adjacency length mismatch");
        for (fi, tri) in m.faces.iter().enumerate() {
            for &v in tri {
                assert!((v as usize) < m.verts.len(), "face references dead vertex");
                assert!(
                    m.vfaces[v as usize].contains(&(fi as u32)),
                    "face {fi} missing from vfaces[{v}]"
                );
            }
        }
        for (v, fl) in m.vfaces.iter().enumerate() {
            for &f in fl {
                assert!((f as usize) < m.faces.len(), "vfaces references dead face");
                assert!(
                    m.faces[f as usize].contains(&(v as u32)),
                    "vfaces[{v}] points at face {f} that does not use it"
                );
            }
        }
    }

    #[test]
    fn icosphere_is_a_closed_manifold() {
        let m = primitives::icosphere(3);
        validate(&m);
        // Euler characteristic of a sphere: V - E + F = 2, with E = 3F/2.
        let v = m.vert_count() as i64;
        let f = m.face_count() as i64;
        assert_eq!(f % 2, 0);
        assert_eq!(v - (3 * f / 2) + f, 2);
    }

    #[test]
    fn split_edge_keeps_adjacency_consistent() {
        let mut m = primitives::icosphere(1);
        let (v0, f0) = (m.vert_count(), m.face_count());
        let [a, b, _] = m.faces[0];
        assert!(m.split_edge(a, b).is_some());
        validate(&m);
        assert_eq!(m.vert_count(), v0 + 1);
        assert_eq!(m.face_count(), f0 + 2);
    }

    #[test]
    fn collapse_edge_keeps_adjacency_consistent() {
        let mut m = primitives::icosphere(2);
        let (v0, f0) = (m.vert_count(), m.face_count());
        let [a, b, _] = m.faces[0];
        assert!(m.collapse_edge(a, b), "interior edge should be collapsible");
        validate(&m);
        assert_eq!(m.vert_count(), v0 - 1);
        assert_eq!(m.face_count(), f0 - 2);
    }

    #[test]
    fn dyntopo_refines_then_survives_validation() {
        let mut s = Sculptor::new(primitives::icosphere(2));
        s.dyntopo.detail = s.mesh.mean_edge_len() * 0.4;
        let before = s.mesh.face_count();
        let hit = StrokeInput {
            point: Vec3::new(0.0, 1.0, 0.0),
            normal: Vec3::Y,
            drag: Vec3::ZERO,
        };
        s.begin_stroke();
        for _ in 0..4 {
            s.stroke(&hit);
        }
        s.end_stroke();
        validate(&s.mesh);
        assert!(s.mesh.face_count() > before, "dyntopo should have subdivided");
    }

    #[test]
    fn undo_restores_the_previous_state() {
        let mut s = Sculptor::new(primitives::icosphere(2));
        let before = s.mesh.verts[0].pos;
        s.begin_stroke();
        s.stroke(&StrokeInput {
            point: s.mesh.verts[0].pos,
            normal: Vec3::Y,
            drag: Vec3::ZERO,
        });
        s.end_stroke();
        assert_ne!(s.mesh.verts[0].pos, before);
        s.undo();
        validate(&s.mesh);
        assert_eq!(s.mesh.verts[0].pos, before);
    }

    #[test]
    fn raycast_hits_the_unit_sphere() {
        let m = primitives::icosphere(3);
        let hit = query::raycast(&m, Vec3::new(0.0, 0.0, 5.0), -Vec3::Z).expect("should hit");
        assert!((hit.point.z - 1.0).abs() < 0.05, "hit at {:?}", hit.point);
        assert!(hit.normal.dot(Vec3::Z) > 0.8, "outward normal expected");
    }

    #[test]
    fn obj_roundtrip_preserves_geometry() {
        let m = primitives::icosphere(2);
        let path = std::env::temp_dir().join("sculpt_rs_roundtrip.obj");
        io::write_obj(&m, &path, true).unwrap();
        let back = io::read_obj(&path).unwrap();
        assert_eq!(m.vert_count(), back.vert_count());
        assert_eq!(m.face_count(), back.face_count());
        let _ = std::fs::remove_file(&path);
    }
}
