//! Core sculpting engine: scene, mesh, dynamic topology, brushes, topology
//! commands, history and I/O.
//!
//! Free of any graphics dependency so it can be driven by the desktop app, a
//! headless test, or a future wasm build.

pub mod accel;
pub mod brush;
pub mod dyntopo;
pub mod history;
pub mod io;
pub mod mesh;
pub mod primitives;
pub mod query;
pub mod scene;
pub mod topology;

pub use accel::Grid;
pub use brush::{Axis, BlendMode, Brush, BrushKind, Falloff, FillScope, StrokeInput};
pub use dyntopo::Dyntopo;
pub use history::History;
pub use mesh::{Mesh, Vertex};
pub use query::Hit;
pub use scene::{Object, Scene, Transform};
pub use topology::RemeshOptions;

use glam::Vec3;

/// A pick resolved against the whole scene.
#[derive(Clone, Copy, Debug)]
pub struct SceneHit {
    pub object: usize,
    /// Hit in world space.
    pub world: Hit,
    /// The same hit in the object's local space, which is where sculpting
    /// happens.
    pub local: Hit,
}

/// Drives a scene through strokes, keeping topology, normals and undo in sync.
pub struct Sculptor {
    pub scene: Scene,
    pub brush: Brush,
    /// Per-tool settings, so switching brushes restores what you had.
    presets: Vec<Brush>,
    pub dyntopo: Dyntopo,
    pub dyntopo_enabled: bool,
    pub symmetry: bool,
    pub symmetry_axis: Axis,
    pub history: History,
    /// Set when vertex data changed; the renderer clears it after upload.
    pub verts_dirty: bool,
    /// Set when the index buffer needs a re-upload.
    pub topology_dirty: bool,
    stroking: bool,
    stroke_state: brush::StrokeState,
}

impl Sculptor {
    pub fn new(mesh: Mesh) -> Self {
        Self::with_scene(Scene::with_object(Object::new("sculpt", mesh)))
    }

    pub fn with_scene(scene: Scene) -> Self {
        let brush = Brush::default();
        let mut s = Self {
            presets: BrushKind::ALL.iter().map(|&k| Brush::defaults_for(k)).collect(),
            brush,
            dyntopo: Dyntopo::default(),
            dyntopo_enabled: true,
            symmetry: true,
            symmetry_axis: Axis::X,
            history: History::default(),
            verts_dirty: true,
            topology_dirty: true,
            stroking: false,
            stroke_state: brush::StrokeState::default(),
            scene,
        };
        s.brush = s.presets[Self::preset_index(BrushKind::Clay)];
        s.reset_detail_to_mesh();
        s
    }

    fn preset_index(kind: BrushKind) -> usize {
        BrushKind::ALL.iter().position(|&k| k == kind).unwrap_or(0)
    }

    /// Switches tool, remembering the settings of the one being left.
    pub fn set_brush_kind(&mut self, kind: BrushKind) {
        if self.brush.kind == kind {
            return;
        }
        self.presets[Self::preset_index(self.brush.kind)] = self.brush;
        self.brush = self.presets[Self::preset_index(kind)];
        self.brush.kind = kind;
    }

    pub fn reset_brush_presets(&mut self) {
        self.presets = BrushKind::ALL.iter().map(|&k| Brush::defaults_for(k)).collect();
        self.brush = self.presets[Self::preset_index(self.brush.kind)];
    }

    // ---- active object ------------------------------------------------------

    pub fn mesh(&self) -> &Mesh {
        static EMPTY: std::sync::OnceLock<Mesh> = std::sync::OnceLock::new();
        self.scene
            .active()
            .map(|o| &o.mesh)
            .unwrap_or_else(|| EMPTY.get_or_init(Mesh::new))
    }

    pub fn mesh_mut(&mut self) -> Option<&mut Mesh> {
        self.scene.active_mut().map(|o| &mut o.mesh)
    }

    pub fn active_index(&self) -> usize {
        self.scene.active
    }

    /// Object-space brush radius, accounting for the object's scale.
    fn local_radius(&self) -> f32 {
        let s = self.scene.active().map(|o| o.transform.mean_scale()).unwrap_or(1.0);
        self.brush.radius / s.max(1e-6)
    }

    /// Seeds the dyntopo target from the current tessellation so a freshly
    /// loaded mesh does not immediately explode or collapse under the brush.
    pub fn reset_detail_to_mesh(&mut self) {
        let e = self.mesh().mean_edge_len();
        if e.is_finite() && e > 0.0 {
            self.dyntopo.detail = e;
        }
    }

    pub fn replace_mesh(&mut self, mesh: Mesh) {
        if let Some(o) = self.scene.active_mut() {
            o.mesh = mesh;
            o.transform = Transform::default();
        } else {
            self.scene.add(Object::new("sculpt", mesh));
        }
        self.history.clear();
        self.reset_detail_to_mesh();
        self.mark_all_dirty();
    }

    pub fn mark_all_dirty(&mut self) {
        self.verts_dirty = true;
        self.topology_dirty = true;
    }

    // ---- strokes ------------------------------------------------------------

    pub fn begin_stroke(&mut self) {
        if !self.stroking {
            let idx = self.scene.active;
            self.history.push_geometry(&self.scene, idx);
            self.stroking = true;
            self.stroke_state = brush::StrokeState::default();
        }
    }

    pub fn end_stroke(&mut self) {
        self.stroking = false;
    }

    pub fn is_stroking(&self) -> bool {
        self.stroking
    }

    /// One dab in world space, including its mirrored twin when symmetry is on.
    pub fn stroke(&mut self, world: &StrokeInput) {
        let Some(obj) = self.scene.active() else { return };
        let local = if obj.transform.is_identity() {
            *world
        } else {
            let inv = obj.transform.inverse_matrix();
            StrokeInput {
                point: inv.transform_point3(world.point),
                normal: inv.transform_vector3(world.normal).normalize_or(Vec3::Y),
                drag: inv.transform_vector3(world.drag),
                view_dir: inv.transform_vector3(world.view_dir).normalize_or(-Vec3::Z),
                ..*world
            }
        };
        self.stroke_local(&local);
    }

    /// One dab already expressed in the active object's space.
    pub fn stroke_local(&mut self, input: &StrokeInput) {
        self.dab(input);
        if self.symmetry {
            let m = input.mirrored(self.symmetry_axis, 0.0);
            self.dab(&m);
        }
    }

    fn dab(&mut self, input: &StrokeInput) {
        let deforms = self.brush.kind.deforms();
        let radius = self.local_radius();
        let brush = Brush { radius, ..self.brush };
        let dyn_params = self.dyntopo;
        let dyn_on = self.dyntopo_enabled;
        let point = input.point;

        let Some(mesh) = self.mesh_mut() else { return };
        // The grid is sized from the query radius, so tell it before we query.
        mesh.ensure_accel(radius);

        if deforms && dyn_on {
            if dyntopo::refine(mesh, point, radius, &dyn_params) {
                self.topology_dirty = true;
            }
        }

        let mut state = self.stroke_state;
        let Some(mesh) = self.mesh_mut() else { return };
        let touched = brush::apply(mesh, &brush, input, &mut state);
        if !touched.is_empty() {
            if deforms {
                // Normals change one ring further out than the positions did,
                // and `update_normals` records that wider set itself.
                mesh.update_normals(&touched);
            } else {
                mesh.commit_attributes(&touched);
            }
            self.verts_dirty = true;
        }
        self.stroke_state = state;
    }

    /// Hands over what the active mesh changed since the last call.
    pub fn take_dirty(&mut self) -> (Vec<u32>, Vec<u32>, bool) {
        match self.mesh_mut() {
            Some(m) => m.take_dirty(),
            None => (Vec::new(), Vec::new(), true),
        }
    }

    /// Floods colour from a picked face. Fill is a click tool, so it records
    /// its own undo step rather than riding on a stroke.
    pub fn fill_at(&mut self, hit: &SceneHit) -> usize {
        if self.brush.kind != BrushKind::Fill {
            return 0;
        }
        let b = self.brush;
        self.select(hit.object);
        self.checkpoint();
        let face = hit.local.face;
        let n = self
            .mesh_mut()
            .map(|m| {
                topology::fill(
                    m,
                    face,
                    b.fill_scope,
                    b.paint_color,
                    b.blend,
                    b.strength,
                    b.fill_angle,
                )
            })
            .unwrap_or(0);
        self.verts_dirty = true;
        n
    }

    // ---- history ------------------------------------------------------------

    pub fn undo(&mut self) {
        if self.history.undo(&mut self.scene) {
            self.invalidate_all_accel();
            self.mark_all_dirty();
        }
    }

    pub fn redo(&mut self) {
        if self.history.redo(&mut self.scene) {
            self.invalidate_all_accel();
            self.mark_all_dirty();
        }
    }

    fn invalidate_all_accel(&mut self) {
        for o in &mut self.scene.objects {
            o.mesh.invalidate_accel();
        }
    }

    /// Records the active object's geometry so a one-shot command can be undone.
    fn checkpoint(&mut self) {
        let idx = self.scene.active;
        self.history.push_geometry(&self.scene, idx);
    }

    // ---- mask ---------------------------------------------------------------

    pub fn clear_mask(&mut self) {
        self.checkpoint();
        if let Some(m) = self.mesh_mut() {
            topology::clear_mask(m);
        }
        self.verts_dirty = true;
    }

    pub fn invert_mask(&mut self) {
        self.checkpoint();
        if let Some(m) = self.mesh_mut() {
            topology::invert_mask(m);
        }
        self.verts_dirty = true;
    }

    /// Positive blurs, negative sharpens.
    pub fn filter_mask(&mut self, amount: f32) {
        self.checkpoint();
        if let Some(m) = self.mesh_mut() {
            topology::filter_mask(m, amount);
        }
        self.verts_dirty = true;
    }

    /// Lifts the masked region into a new object.
    pub fn extract_masked(&mut self, thickness: f32) -> bool {
        let Some(extracted) = self.mesh().pipe_extract(thickness) else {
            return false;
        };
        self.history.push_scene(&self.scene);
        self.scene.add(Object::new("extracted", extracted));
        self.mark_all_dirty();
        true
    }

    // ---- topology commands --------------------------------------------------

    /// Splits every triangle into four.
    ///
    /// Refuses when the result would not fit: one subdivision quadruples the
    /// triangles and roughly quadruples the memory, and the allocation failure
    /// that follows is an abort, not something a caller can recover from. The
    /// ceiling is the same one dynamic topology respects.
    pub fn subdivide(&mut self, smooth: bool) -> Result<usize, String> {
        let (verts, faces) = (self.mesh().vert_count(), self.mesh().face_count());
        // A closed mesh has three halves of an edge per face, and every edge
        // gains a vertex.
        let projected_verts = verts + faces * 3 / 2;
        let ceiling = self.dyntopo.max_verts;
        if projected_verts > ceiling {
            return Err(format!(
                "subdividing would reach {:.1} M vertices, over the {:.1} M ceiling",
                projected_verts as f64 / 1.0e6,
                ceiling as f64 / 1.0e6
            ));
        }
        self.checkpoint();
        if let Some(o) = self.scene.active_mut() {
            o.mesh = topology::subdivide(&o.mesh, smooth);
        }
        self.reset_detail_to_mesh();
        self.mark_all_dirty();
        Ok(self.mesh().face_count())
    }

    pub fn decimate(&mut self, ratio: f32) {
        self.checkpoint();
        if let Some(m) = self.mesh_mut() {
            topology::decimate(m, ratio);
        }
        self.reset_detail_to_mesh();
        self.mark_all_dirty();
    }

    pub fn voxel_remesh(&mut self, opts: &RemeshOptions) {
        self.checkpoint();
        if let Some(o) = self.scene.active_mut() {
            o.mesh = topology::voxel_remesh(&o.mesh, opts);
        }
        self.reset_detail_to_mesh();
        self.mark_all_dirty();
    }

    pub fn close_holes(&mut self) -> usize {
        self.checkpoint();
        let n = self.mesh_mut().map(topology::close_holes).unwrap_or(0);
        self.mark_all_dirty();
        n
    }

    pub fn smooth_all(&mut self, amount: f32) {
        self.checkpoint();
        if let Some(m) = self.mesh_mut() {
            topology::laplacian_smooth(m, amount);
            m.recompute_normals();
        }
        self.verts_dirty = true;
    }

    pub fn mirror(&mut self, axis: Axis) {
        self.checkpoint();
        if let Some(m) = self.mesh_mut() {
            topology::mirror(m, axis);
        }
        self.mark_all_dirty();
    }

    pub fn symmetrize(&mut self, axis: Axis, keep_positive: bool) {
        self.checkpoint();
        if let Some(o) = self.scene.active_mut() {
            o.mesh = topology::symmetrize(&o.mesh, axis, keep_positive);
        }
        self.mark_all_dirty();
    }

    // ---- scene --------------------------------------------------------------

    pub fn add_object(&mut self, name: &str, mesh: Mesh) {
        self.history.push_scene(&self.scene);
        self.scene.add(Object::new(name, mesh));
        self.reset_detail_to_mesh();
        self.mark_all_dirty();
    }

    pub fn delete_object(&mut self, index: usize) {
        if self.scene.objects.len() <= 1 {
            return;
        }
        self.history.push_scene(&self.scene);
        self.scene.remove(index);
        self.mark_all_dirty();
    }

    pub fn duplicate_object(&mut self, index: usize) {
        self.history.push_scene(&self.scene);
        self.scene.duplicate(index);
        self.mark_all_dirty();
    }

    pub fn merge_visible(&mut self) {
        if self.scene.objects.len() < 2 {
            return;
        }
        self.history.push_scene(&self.scene);
        self.scene.merge_visible();
        self.reset_detail_to_mesh();
        self.mark_all_dirty();
    }

    pub fn select(&mut self, index: usize) {
        if index < self.scene.objects.len() && index != self.scene.active {
            self.scene.active = index;
            self.reset_detail_to_mesh();
            self.mark_all_dirty();
        }
    }

    /// Bakes the active object's transform into its vertices.
    pub fn apply_transform(&mut self) {
        let idx = self.scene.active;
        self.history.push_geometry(&self.scene, idx);
        if let Some(o) = self.scene.active_mut() {
            o.apply_transform();
        }
        self.mark_all_dirty();
    }

    // ---- picking ------------------------------------------------------------

    /// Casts a world-space ray at every visible object and keeps the nearest.
    pub fn pick(&self, origin: Vec3, dir: Vec3) -> Option<SceneHit> {
        let mut best: Option<SceneHit> = None;
        for (i, o) in self.scene.objects.iter().enumerate() {
            if !o.visible || o.mesh.faces.is_empty() {
                continue;
            }
            let (lo, ld, m) = if o.transform.is_identity() {
                (origin, dir, None)
            } else {
                let inv = o.transform.inverse_matrix();
                (
                    inv.transform_point3(origin),
                    inv.transform_vector3(dir),
                    Some(o.transform.matrix()),
                )
            };
            let Some(local) = query::raycast(&o.mesh, lo, ld) else {
                continue;
            };
            let world = match m {
                None => local,
                Some(m) => {
                    let p = m.transform_point3(local.point);
                    let n = m.transform_vector3(local.normal).normalize_or(Vec3::Y);
                    Hit { face: local.face, point: p, normal: n, t: (p - origin).dot(dir) }
                }
            };
            if best.as_ref().is_none_or(|b| world.t < b.world.t) {
                best = Some(SceneHit { object: i, world, local });
            }
        }
        best
    }

    /// Like [`Sculptor::pick`], but when the ray misses everything it reaches
    /// for the nearest surface within `reach` of the ray.
    ///
    /// Sculpting the edge of a form means putting the cursor just outside it,
    /// where a strict ray cast reports nothing. `reach` is normally the brush
    /// radius, so the rule is simply: if the brush would touch the surface, the
    /// stroke works.
    pub fn pick_soft(&self, origin: Vec3, dir: Vec3, reach: f32) -> Option<SceneHit> {
        if let Some(hit) = self.pick(origin, dir) {
            return Some(hit);
        }
        let mut best: Option<SceneHit> = None;
        for (i, o) in self.scene.objects.iter().enumerate() {
            if !o.visible || o.mesh.verts.is_empty() {
                continue;
            }
            let identity = o.transform.is_identity();
            let (lo, ld, scale) = if identity {
                (origin, dir, 1.0)
            } else {
                let inv = o.transform.inverse_matrix();
                (
                    inv.transform_point3(origin),
                    inv.transform_vector3(dir).normalize_or(-Vec3::Z),
                    o.transform.mean_scale().max(1e-6),
                )
            };
            let Some(local) = query::nearest_to_ray(&o.mesh, lo, ld, reach / scale) else {
                continue;
            };
            let world = if identity {
                local
            } else {
                let m = o.transform.matrix();
                let p = m.transform_point3(local.point);
                Hit {
                    face: local.face,
                    point: p,
                    normal: m.transform_vector3(local.normal).normalize_or(Vec3::Y),
                    t: (p - origin).dot(dir),
                }
            };
            if best.as_ref().is_none_or(|b| world.t < b.world.t) {
                best = Some(SceneHit { object: i, world, local });
            }
        }
        best
    }

    /// Colour under the cursor, for the eyedropper.
    pub fn sample_color(&self, hit: &SceneHit) -> Option<Vec3> {
        let mesh = &self.scene.objects.get(hit.object)?.mesh;
        let tri = *mesh.faces.get(hit.local.face as usize)?;
        let mut best = (f32::MAX, tri[0]);
        for &v in &tri {
            let d = mesh.verts[v as usize].pos.distance_squared(hit.local.point);
            if d < best.0 {
                best = (d, v);
            }
        }
        Some(mesh.verts[best.1 as usize].col)
    }
}

/// Small extension so `Sculptor::extract_masked` reads cleanly.
trait ExtractExt {
    fn pipe_extract(&self, thickness: f32) -> Option<Mesh>;
}

impl ExtractExt for Mesh {
    fn pipe_extract(&self, thickness: f32) -> Option<Mesh> {
        topology::extract_masked(self, thickness)
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

    /// Checks that the spatial grid still agrees with the geometry.
    fn validate_accel(m: &Mesh) {
        let Some(g) = &m.accel else { return };
        let r = m.mean_edge_len() * 3.0;
        for (i, v) in m.verts.iter().enumerate().step_by(7) {
            let found = g.verts_in_sphere(m, v.pos, r).expect("grid query");
            assert!(
                found.contains(&(i as u32)),
                "vertex {i} missing from its own neighbourhood"
            );
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
        s.dyntopo.detail = s.mesh().mean_edge_len() * 0.4;
        let before = s.mesh().face_count();
        let hit = StrokeInput {
            point: Vec3::new(0.0, 1.0, 0.0),
            normal: Vec3::Y,
            ..Default::default()
        };
        s.begin_stroke();
        for _ in 0..4 {
            s.stroke(&hit);
        }
        s.end_stroke();
        validate(s.mesh());
        validate_accel(s.mesh());
        assert!(s.mesh().face_count() > before, "dyntopo should have subdivided");
    }

    #[test]
    fn grid_and_brute_force_agree() {
        let mut m = primitives::icosphere(3);
        let center = Vec3::new(0.0, 1.0, 0.0);
        let r = 0.35;
        let mut brute = query::verts_in_sphere(&m, center, r);
        m.ensure_accel(r);
        let mut fast = query::verts_in_sphere(&m, center, r);
        brute.sort_unstable();
        fast.sort_unstable();
        assert_eq!(brute, fast);

        let o = Vec3::new(0.0, 0.0, 5.0);
        let d = -Vec3::Z;
        let fast_hit = query::raycast(&m, o, d).expect("grid hit");
        m.invalidate_accel();
        let slow_hit = query::raycast(&m, o, d).expect("brute hit");
        assert!((fast_hit.t - slow_hit.t).abs() < 1e-4, "{fast_hit:?} vs {slow_hit:?}");
    }

    #[test]
    fn undo_restores_the_previous_state() {
        let mut s = Sculptor::new(primitives::icosphere(2));
        let before = s.mesh().verts[0].pos;
        s.begin_stroke();
        s.stroke(&StrokeInput {
            point: before,
            normal: Vec3::Y,
            ..Default::default()
        });
        s.end_stroke();
        assert_ne!(s.mesh().verts[0].pos, before);
        s.undo();
        validate(s.mesh());
        assert_eq!(s.mesh().verts[0].pos, before);
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

    #[test]
    fn ply_roundtrip_preserves_colors() {
        let mut m = primitives::icosphere(2);
        m.verts[3].col = Vec3::new(1.0, 0.0, 0.0);
        let path = std::env::temp_dir().join("sculpt_rs_roundtrip.ply");
        io::write_ply(&m, &path).unwrap();
        let back = io::read_ply(&path).unwrap();
        assert_eq!(m.vert_count(), back.vert_count());
        assert_eq!(m.face_count(), back.face_count());
        assert!(back.verts[3].col.x > 0.9 && back.verts[3].col.y < 0.1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scene_roundtrip_preserves_objects() {
        let mut scene = Scene::with_object(Object::new("a", primitives::icosphere(2)));
        let mut second = Object::new("b", primitives::cube(4));
        second.transform.position = Vec3::new(2.0, 0.0, 0.0);
        scene.add(second);
        let path = std::env::temp_dir().join("sculpt_rs_roundtrip.sculpt");
        io::write_scene(&scene, &path).unwrap();
        let back = io::read_scene(&path).unwrap();
        assert_eq!(back.objects.len(), 2);
        assert_eq!(back.objects[1].name, "b");
        assert_eq!(back.objects[1].transform.position.x, 2.0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn subdivision_quadruples_faces() {
        let m = primitives::icosphere(1);
        let s = topology::subdivide(&m, true);
        validate(&s);
        assert_eq!(s.face_count(), m.face_count() * 4);
    }

    #[test]
    fn decimation_reduces_and_stays_valid() {
        let mut m = primitives::icosphere(3);
        let before = m.face_count();
        topology::decimate(&mut m, 0.5);
        validate(&m);
        assert!(m.face_count() < before);
    }

    #[test]
    fn voxel_remesh_produces_a_closed_shell() {
        let m = primitives::icosphere(3);
        let out = topology::voxel_remesh(&m, &RemeshOptions { resolution: 32, smoothing: 1, transfer_colors: false });
        validate(&out);
        assert!(out.face_count() > 100, "got {} faces", out.face_count());
        assert!(out.boundary_loops().is_empty(), "remesh should be watertight");
    }

    #[test]
    fn close_holes_seals_an_open_plane() {
        let mut m = primitives::plane(4);
        assert!(!m.boundary_loops().is_empty());
        let filled = topology::close_holes(&mut m);
        validate(&m);
        assert_eq!(filled, 1);
        assert!(m.boundary_loops().is_empty());
    }

    #[test]
    fn symmetry_mirrors_the_stroke() {
        let mut s = Sculptor::new(primitives::icosphere(3));
        s.symmetry = true;
        s.symmetry_axis = Axis::X;
        s.dyntopo_enabled = false;
        s.brush = Brush { kind: BrushKind::Draw, radius: 0.3, strength: 0.8, ..Brush::default() };
        let p = Vec3::new(0.8, 0.0, 0.0).normalize();
        s.begin_stroke();
        s.stroke(&StrokeInput { point: p, normal: p, ..Default::default() });
        s.end_stroke();

        let far_right = s.mesh().verts.iter().map(|v| v.pos.length()).fold(0.0f32, f32::max);
        let left_max = s
            .mesh()
            .verts
            .iter()
            .filter(|v| v.pos.x < -0.5)
            .map(|v| v.pos.length())
            .fold(0.0f32, f32::max);
        assert!(left_max > 1.0, "the mirrored side should have moved too");
        assert!((far_right - left_max).abs() < 1e-3, "both sides should match");
    }

    #[test]
    fn blend_modes_go_the_right_way() {
        let grey = Vec3::splat(0.5);
        let half = Vec3::splat(0.5);
        assert!(BlendMode::Multiply.apply(grey, half).x < grey.x);
        assert!(BlendMode::Screen.apply(grey, half).x > grey.x);
        assert!(BlendMode::Add.apply(grey, half).x > grey.x);
        assert!(BlendMode::Subtract.apply(grey, half).x < grey.x);
        assert_eq!(BlendMode::Normal.apply(grey, Vec3::X), Vec3::X);
        // Hue keeps the surface luminance.
        let lum = |c: Vec3| 0.299 * c.x + 0.587 * c.y + 0.114 * c.z;
        let out = BlendMode::Hue.apply(grey, Vec3::new(0.2, 0.4, 0.9));
        assert!((lum(out) - lum(grey)).abs() < 0.02);
    }

    #[test]
    fn region_fill_stops_at_a_crease() {
        // A cube face is flat, and every edge around it turns ninety degrees,
        // so a region fill should colour one side and go no further.
        let mut m = primitives::cube(4);
        let touched = topology::fill(
            &mut m,
            0,
            FillScope::Face,
            Vec3::X,
            BlendMode::Normal,
            1.0,
            35.0,
        );
        assert_eq!(touched, 3, "a face fill covers one triangle");

        let mut m = primitives::cube(4);
        let touched = topology::fill(
            &mut m,
            0,
            FillScope::Region,
            Vec3::X,
            BlendMode::Normal,
            1.0,
            35.0,
        );
        // One side of a 4x4 subdivided cube has five by five vertices.
        assert_eq!(touched, 25, "the fill should stop at the cube's edges");
        let red = m.verts.iter().filter(|v| v.col.x > 0.9 && v.col.y < 0.1).count();
        assert_eq!(red, 25);
    }

    #[test]
    fn masked_vertices_resist_a_fill() {
        let mut m = primitives::cube(4);
        for v in m.verts.iter_mut() {
            v.mask = 1.0;
        }
        let before = m.verts[0].col;
        topology::fill(&mut m, 0, FillScope::Object, Vec3::X, BlendMode::Normal, 1.0, 35.0);
        assert_eq!(m.verts[0].col, before);
    }

    #[test]
    fn switching_tools_remembers_settings() {
        let mut s = Sculptor::new(primitives::icosphere(1));
        s.set_brush_kind(BrushKind::Draw);
        s.brush.strength = 0.123;
        s.set_brush_kind(BrushKind::Smooth);
        assert_ne!(s.brush.strength, 0.123);
        s.set_brush_kind(BrushKind::Draw);
        assert_eq!(s.brush.strength, 0.123);
    }
}
