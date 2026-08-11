//! Core sculpting engine: scene, mesh, dynamic topology, brushes, topology
//! commands, history and I/O.
//!
//! Free of any graphics dependency so it can be driven by the desktop app, a
//! headless test, or a future wasm build.

pub mod accel;
pub mod alpha;
pub mod brush;
pub mod channel;
pub mod cluster;
pub mod dyntopo;
pub mod history;
pub mod io;
pub mod mesh;
pub mod primitives;
pub mod query;
pub mod scene;
pub mod topology;
pub mod voxel;

pub use accel::Grid;
pub use alpha::{Alpha, Shape as AlphaShape};
pub use channel::Channel;
pub use cluster::{Cluster, Partition};
pub use brush::{Axis, BlendMode, Brush, BrushKind, Falloff, FillScope, StrokeInput};
pub use dyntopo::{DetailMode, Dyntopo};
pub use history::History;
pub use mesh::{Mesh, Vertex};
pub use query::Hit;
pub use scene::{Object, Scene, Transform};
pub use topology::RemeshOptions;
pub use voxel::VoxelField;

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
    /// Every alpha the brush can stamp through. Brushes point into this by
    /// index; the reference count is only there so a dab can hold one while the
    /// mesh is borrowed.
    pub alphas: Vec<std::sync::Arc<Alpha>>,
    pub dyntopo: Dyntopo,
    pub dyntopo_enabled: bool,
    pub symmetry: bool,
    pub symmetry_axis: Axis,
    pub history: History,
    /// Set when vertex data changed; the renderer clears it after upload.
    pub verts_dirty: bool,
    /// Set when the index buffer needs a re-upload.
    pub topology_dirty: bool,
    /// Most vertices the GPU will hold, when the caller knows. The engine has
    /// no graphics dependency, so it is told rather than asking, and `None`
    /// simply means no check.
    pub max_gpu_verts: Option<usize>,
    /// When set, the active object sculpts through the voxel field rather than
    /// the mesh: a dab stamps the field and the surface is extracted back into
    /// the mesh. The cost stops depending on the polygon count.
    pub voxel_field: Option<VoxelField>,
    pub voxel_mode: bool,
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
            alphas: alpha::Shape::ALL
                .iter()
                .map(|&s| std::sync::Arc::new(Alpha::procedural(s, 256)))
                .collect(),
            brush,
            dyntopo: Dyntopo::default(),
            dyntopo_enabled: true,
            symmetry: true,
            symmetry_axis: Axis::X,
            history: History::default(),
            verts_dirty: true,
            topology_dirty: true,
            max_gpu_verts: None,
            voxel_field: None,
            voxel_mode: false,
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

    // ---- voxel sculpting ----------------------------------------------------

    pub fn voxel_mode(&self) -> bool {
        self.voxel_mode
    }

    /// Voxelises the active mesh into a field at `h` voxels a side of world
    /// units, and replaces the mesh with the extracted surface. After this the
    /// brush stamps the field and the surface is re-extracted, so the cost of
    /// a dab depends on the voxel size, not the mesh it came from.
    pub fn voxelize_active(&mut self, h: f32) {
        let mesh = self.mesh();
        if mesh.faces.is_empty() {
            return;
        }
        let mut field = VoxelField::from_mesh(mesh, h);
        field.extract();
        self.voxel_field = Some(field);
        self.voxel_mode = true;
        if let Some(o) = self.scene.active_mut() {
            o.mesh = self.voxel_field.as_ref().unwrap().surface().clone();
        }
        self.mark_all_dirty();
    }

    /// Voxelises at a resolution in voxels across the model's largest side.
    pub fn voxelize_active_res(&mut self, res: usize) {
        let (lo, hi) = self.mesh().bounds();
        let extent = (hi - lo).max_element().max(1e-6);
        self.voxelize_active(extent / res.max(16) as f32);
    }

    /// One voxel dab: stamps the field and extracts the surface back into the
    /// active mesh. Returns false when no field is live.
    pub fn voxel_dab(&mut self, point: Vec3, radius: f32, amount: f32) -> bool {
        let Some(field) = &mut self.voxel_field else {
            return false;
        };
        field.stamp(point, radius, amount);
        field.extract_modified();
        let (dirty_v, dirty_f, fully) = field.take_dirty();
        let active = self.scene.active;
        if let Some(o) = self.scene.objects.get_mut(active) {
            o.mesh.sync_from(field.surface(), &dirty_v, &dirty_f, fully);
        }
        self.mark_all_dirty();
        true
    }

    /// Leaves voxel mode, keeping the extracted surface as the object's mesh.
    pub fn exit_voxel(&mut self) {
        self.voxel_field = None;
        self.voxel_mode = false;
    }

    // ---- strokes ------------------------------------------------------------

    /// Opens a stroke.
    ///
    /// No copy is taken here. A stroke moves a few thousand vertices out of
    /// millions, so it is remembered as the slots it writes over, recorded as
    /// it writes them. That holds whether or not it cuts the topology, so
    /// there is nothing to fall back to.
    pub fn begin_stroke(&mut self) {
        if !self.stroking {
            self.history.fit_budget_to(self.mesh_bytes());
            // The one moment worth paying for a grid sized to this brush: the
            // radius is settled, and every dab that follows will be spared the
            // rebuild.
            let radius = self.local_radius();
            if let Some(mesh) = self.mesh_mut() {
                mesh.ensure_accel(radius);
                mesh.begin_log();
            }
            self.stroking = true;
            self.stroke_state = brush::StrokeState::default();
        }
    }

    pub fn end_stroke(&mut self) {
        self.stroking = false;
        let active = self.scene.active;
        if let Some(log) = self.mesh_mut().and_then(|m| m.take_log()) {
            self.history.push_topology(active, log);
        }
    }

    /// Roughly what the active mesh occupies, for sizing the history against it.
    fn mesh_bytes(&self) -> usize {
        let m = self.mesh();
        m.pos.len() * std::mem::size_of::<Vertex>() + m.faces.len() * 12
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
                view_right: inv.transform_vector3(world.view_right).normalize_or(Vec3::X),
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
        // The faces the dab disturbed go back in their cells here, once, rather
        // than three times each in the middle of it.
        if let Some(mesh) = self.mesh_mut() {
            mesh.flush_refit();
        }
    }

    fn dab(&mut self, input: &StrokeInput) {
        if self.voxel_mode {
            // The mesh is not deformed in voxel mode: the brush stamps the
            // field, and the extracted surface becomes the mesh. The cost of
            // the dab depends on the voxel size, not the polygon count.
            let radius = self.local_radius();
            let sign = if self.brush.negative && self.brush.kind.has_negative() {
                -1.0
            } else {
                1.0
            };
            let amount = sign * self.brush.strength * 0.6;
            self.voxel_dab(input.point, radius, amount);
            return;
        }
        let deforms = self.brush.kind.deforms();
        let radius = self.local_radius();
        let brush = Brush { radius, ..self.brush };
        // The detail setting is a number in whatever unit the mode names, so it
        // is turned into an edge length here, where the brush radius and the
        // scale of a pixel are both known.
        let dyn_params = Dyntopo {
            detail: self.dyntopo.target_edge(radius, input.world_per_pixel),
            ..self.dyntopo
        };
        let dyn_on = self.dyntopo_enabled;
        let point = input.point;

        {
            let stroking = self.stroking;
            let Some(mesh) = self.mesh_mut() else { return };
            // The grid is sized from the query radius, so tell it before we
            // query. Mid-stroke it is only built when there is none: resizing
            // it there would stop the pen dead, and `begin_stroke` has already
            // sized it for this brush.
            if stroking {
                mesh.ensure_accel_if_missing(radius);
            } else {
                mesh.ensure_accel(radius);
            }
        }

        if deforms && dyn_on {
            // Planning first and applying the plan, rather than looking for the
            // same edges twice. The undo log follows splits and collapses on
            // its own, so a cut no longer costs a copy of the whole mesh.
            let plan = {
                let mesh = self.mesh();
                dyntopo::plan(mesh, point, radius, &dyn_params)
            };
            let Some(mesh) = self.mesh_mut() else { return };
            if dyntopo::apply(mesh, plan, point, radius, &dyn_params) {
                self.topology_dirty = true;
            }
        }

        let mut state = self.stroke_state;
        // Held by the handle, so the mesh can be borrowed at the same time.
        let alpha = brush
            .alpha
            .and_then(|i| self.alphas.get(i as usize))
            .cloned();
        let Some(mesh) = self.mesh_mut() else { return };
        let touched = brush::apply(mesh, &brush, input, &mut state, alpha.as_deref());
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
        // A vertex buffer the card will not take is a validation error, and a
        // validation error on this path takes the window with it. Better to say
        // no here, with the number that says why.
        if let Some(limit) = self.max_gpu_verts {
            if projected_verts > limit {
                return Err(format!(
                    "subdividing would reach {:.1} M vertices, and this GPU takes {:.1} M at most",
                    projected_verts as f64 / 1.0e6,
                    limit as f64 / 1.0e6
                ));
            }
        }
        // The history is the one thing here that can be given up, and the peak
        // of this operation is the moment it is worth the most.
        self.history.release();
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
            if !o.visible || o.mesh.pos.is_empty() {
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
            let d = mesh.pos[v as usize].distance_squared(hit.local.point);
            if d < best.0 {
                best = (d, v);
            }
        }
        Some(mesh.col(best.1))
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

    /// Voxel mode sculpts a mesh by stamping the field, and the surface it
    /// extracts is a closed mesh that a stamp grows.
    #[test]
    fn voxel_mode_sculpts_the_field_and_grows_the_surface() {
        let mut s = Sculptor::new(primitives::icosphere(4));
        s.voxelize_active_res(160);
        let before = s.mesh().face_count();
        assert!(before > 500, "voxelised shell too small: {before}");
        assert!(s.voxel_mode());
        let (lo, hi) = s.mesh().bounds();
        let top = Vec3::new(0.0, hi.y, 0.0);
        for _ in 0..10 {
            assert!(s.voxel_dab(top, (hi.y - lo.y) * 0.08, 0.4));
        }
        assert!(s.mesh().face_count() > before, "stamps did not grow the shell");
        s.exit_voxel();
        assert!(!s.voxel_mode());
        assert!(s.voxel_field.is_none());
    }

    /// Checks that adjacency agrees with the face array in both directions.
    fn validate(m: &Mesh) {
        assert_eq!(m.pos.len(), m.vfaces.len(), "adjacency length mismatch");
        for (fi, tri) in m.faces.iter().enumerate() {
            for &v in tri {
                assert!((v as usize) < m.pos.len(), "face references dead vertex");
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
        for (i, v) in m.vertices().enumerate().step_by(7) {
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

    /// A stamp has to reach the surface. Half the alpha is solid and half is
    /// empty, so the dab must move the vertices under the solid half and leave
    /// the others exactly where they were.
    #[test]
    fn an_alpha_shapes_the_dab() {
        let mut s = Sculptor::new(primitives::icosphere(4));
        // Solid on the right of the image, empty on the left. Wide enough that
        // the two halves are not one long interpolation between two pixels.
        let n = 16;
        let mut data = vec![0.0f32; n * n];
        for y in 0..n {
            for x in n / 2..n {
                data[y * n + x] = 1.0;
            }
        }
        let half = Alpha::new("half", n as u32, n as u32, data);
        s.alphas.push(std::sync::Arc::new(half));
        s.brush.alpha = Some((s.alphas.len() - 1) as u32);
        s.brush.kind = BrushKind::Draw;
        s.brush.radius = 0.5;
        s.brush.strength = 1.0;
        s.symmetry = false;
        // Keep the vertex list still, so a vertex can be compared with itself.
        s.dyntopo_enabled = false;

        let before: Vec<Vec3> = s.mesh().pos.clone();
        s.begin_stroke();
        s.stroke(&StrokeInput {
            point: Vec3::new(0.0, 1.0, 0.0),
            normal: Vec3::Y,
            view_right: Vec3::X,
            ..Default::default()
        });
        s.end_stroke();

        let mut moved_right = 0;
        for (i, v) in s.mesh().vertices().enumerate() {
            let d = (v.pos - before[i]).length();
            // The dab centre is above +Y, and the image runs along +X.
            if before[i].x > 0.2 && before[i].y > 0.7 {
                if d > 1e-5 {
                    moved_right += 1;
                }
            } else if before[i].x < -0.05 {
                assert!(d < 1e-6, "vertex {i} moved where the alpha is empty");
            }
        }
        assert!(moved_right > 0, "the solid half of the alpha did nothing");
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

    /// La recherche de surface proche marche le long du rayon quand la grille
    /// existe et balaye tout sinon. Les deux doivent répondre la même chose.
    #[test]
    fn the_near_miss_search_agrees_with_the_scan() {
        let mut m = primitives::icosphere(4);
        // Des rayons qui frôlent la sphère de près, de loin, et qui la ratent.
        let cases = [
            (Vec3::new(1.02, 0.0, 5.0), -Vec3::Z, 0.2),
            (Vec3::new(0.0, 1.05, 5.0), -Vec3::Z, 0.15),
            (Vec3::new(0.9, 0.9, 5.0), -Vec3::Z, 0.3),
            (Vec3::new(3.0, 0.0, 5.0), -Vec3::Z, 0.2),
            (Vec3::new(0.0, 0.0, 5.0), Vec3::Z, 0.5),
        ];
        for (o, d, reach) in cases {
            m.invalidate_accel();
            let slow = query::nearest_to_ray(&m, o, d, reach);
            m.ensure_accel(reach);
            let fast = query::nearest_to_ray(&m, o, d, reach);
            match (slow, fast) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    // Deux sommets à égalité sont l'un et l'autre corrects; ce
                    // qui compte est la distance au rayon.
                    let off = |h: &query::Hit| {
                        let to = h.point - o;
                        (to - d.normalize() * to.dot(d.normalize())).length()
                    };
                    assert!(
                        (off(&a) - off(&b)).abs() < 1e-4,
                        "rayon depuis {o:?}: {:?} contre {:?}",
                        off(&a),
                        off(&b)
                    );
                }
                (a, b) => panic!("désaccord depuis {o:?}: {a:?} contre {b:?}"),
            }
        }
    }

    #[test]
    fn undo_restores_the_previous_state() {
        let mut s = Sculptor::new(primitives::icosphere(2));
        let before = s.mesh().pos[0];
        s.begin_stroke();
        s.stroke(&StrokeInput {
            point: before,
            normal: Vec3::Y,
            ..Default::default()
        });
        s.end_stroke();
        assert_ne!(s.mesh().pos[0], before);
        s.undo();
        validate(s.mesh());
        assert_eq!(s.mesh().pos[0], before);
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
        m.set_col(3, Vec3::new(1.0, 0.0, 0.0));
        let path = std::env::temp_dir().join("sculpt_rs_roundtrip.ply");
        io::write_ply(&m, &path).unwrap();
        let back = io::read_ply(&path).unwrap();
        assert_eq!(m.vert_count(), back.vert_count());
        assert_eq!(m.face_count(), back.face_count());
        assert!(back.col(3).x > 0.9 && back.col(3).y < 0.1);
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

    /// Un trait qui ne coupe pas la topologie doit se retenir par différence,
    /// et cette différence doit rendre exactement l'état d'avant.
    #[test]
    fn a_stroke_without_a_cut_is_remembered_by_difference() {
        let mut s = Sculptor::new(primitives::icosphere(4));
        s.dyntopo_enabled = false;
        s.symmetry = false;
        s.brush.kind = BrushKind::Draw;
        s.brush.radius = 0.3;
        s.brush.strength = 1.0;

        let before: Vec<Vec3> = s.mesh().pos.clone();
        let geometry_bytes = before.len() * std::mem::size_of::<Vertex>();

        s.begin_stroke();
        for _ in 0..5 {
            s.stroke(&StrokeInput {
                point: Vec3::new(0.0, 1.0, 0.0),
                normal: Vec3::Y,
                ..Default::default()
            });
        }
        s.end_stroke();
        assert!(s.mesh().pos[0].distance(before[0]) >= 0.0);

        // La trace doit être une fraction de la géométrie, pas une copie.
        let kept = s.history.used_bytes();
        assert!(
            kept < geometry_bytes / 4,
            "l'annulation a gardé {kept} octets pour une géométrie de {geometry_bytes}"
        );

        s.undo();
        for (i, v) in s.mesh().vertices().enumerate() {
            assert!(
                v.pos.distance(before[i]) < 1e-6,
                "le sommet {i} n'est pas revenu à sa place"
            );
        }
    }

    /// Un trait qui coupe la topologie ne peut pas se retenir par différence:
    /// les indices ne veulent plus rien dire. Il doit reprendre la copie.
    #[test]
    fn a_stroke_that_cuts_is_undone_slot_by_slot() {
        let mut s = Sculptor::new(primitives::icosphere(3));
        s.symmetry = false;
        s.dyntopo_enabled = true;
        s.dyntopo.detail = s.mesh().mean_edge_len() * 0.35;
        s.brush.kind = BrushKind::Draw;
        s.brush.radius = 0.4;
        s.brush.strength = 0.8;

        let faces_before = s.mesh().faces.clone();
        let verts_before: Vec<Vec3> = s.mesh().pos.clone();
        let mesh_bytes = s.mesh_bytes();

        s.begin_stroke();
        for _ in 0..4 {
            s.stroke(&StrokeInput {
                point: Vec3::new(0.0, 1.0, 0.0),
                normal: Vec3::Y,
                ..Default::default()
            });
        }
        s.end_stroke();
        assert!(s.mesh().face_count() > faces_before.len(), "rien n'a été raffiné");
        let after: Vec<Vec3> = s.mesh().pos.clone();

        // Ce que le trait a coûté à l'historique doit suivre ce qu'il a touché,
        // pas la taille du modèle: c'est toute la raison du journal.
        assert!(
            s.history.used_bytes() < mesh_bytes,
            "l'historique a pris {} octets pour un maillage de {mesh_bytes}",
            s.history.used_bytes()
        );

        s.undo();
        assert_eq!(s.mesh().faces, faces_before, "les faces ne sont pas revenues");
        assert_eq!(s.mesh().vert_count(), verts_before.len());
        for (i, v) in s.mesh().vertices().enumerate() {
            assert!(v.pos.distance(verts_before[i]) < 1e-6, "sommet {i}");
        }
        validate(s.mesh());

        // Et refaire doit rendre exactement ce que l'annulation a repris.
        s.redo();
        assert_eq!(s.mesh().vert_count(), after.len(), "le refait a perdu des sommets");
        for (i, v) in s.mesh().vertices().enumerate() {
            assert!(v.pos.distance(after[i]) < 1e-6, "sommet {i} refait");
        }
        validate(s.mesh());
    }

    /// Un trait qui ne fait que déplacer des sommets doit s'annuler sans
    /// reconstruire l'adjacence, et rendre le maillage au sommet près.
    #[test]
    fn a_stroke_that_only_moves_is_undone_exactly() {
        let mut s = Sculptor::new(primitives::icosphere(4));
        s.symmetry = false;
        s.dyntopo_enabled = false;
        s.brush.kind = BrushKind::Draw;
        s.brush.radius = 0.3;
        s.brush.strength = 0.7;

        let before: Vec<Vertex> = s.mesh().vertices().collect();
        let faces = s.mesh().faces.clone();

        s.begin_stroke();
        for i in 0..5 {
            let p = Vec3::new(i as f32 * 0.03, 1.0, 0.0).normalize();
            s.stroke(&StrokeInput { point: p, normal: p, ..Default::default() });
        }
        s.end_stroke();
        assert!(
            s.mesh().vertices().zip(&before).any(|(a, b)| a.pos != b.pos),
            "le trait n'a rien déplacé"
        );

        s.undo();
        assert_eq!(s.mesh().faces, faces, "la topologie a bougé alors qu'elle ne devait pas");
        for (i, (now, was)) in s.mesh().vertices().zip(&before).enumerate() {
            assert!(now.pos.distance(was.pos) < 1e-6, "sommet {i}");
            assert!(now.nrm.distance(was.nrm) < 1e-4, "normale {i}");
        }
    }

    #[test]
    fn brush_roundtrip_preserves_settings() {
        let mut brush = Brush::defaults_for(BrushKind::Crease);
        brush.radius = 0.123;
        brush.strength = 0.77;
        brush.falloff = brush::Falloff::Sharp;
        brush.blend = BlendMode::Multiply;
        brush.paint_color = Vec3::new(0.2, 0.4, 0.6);
        brush.lock_plane = true;
        brush.alpha_follow = true;
        brush.alpha_angle = 0.5;
        brush.alpha = Some(1);

        let alphas = vec!["Ring".to_string(), "Cracks".to_string()];
        let saved = vec![io::NamedBrush { name: "My chisel".into(), brush }];
        let path = std::env::temp_dir().join("sculpt_rs_roundtrip.brushes");
        io::write_brushes(&saved, &alphas, &path).unwrap();
        let back = io::read_brushes(&path, &alphas).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "My chisel");
        let b = back[0].brush;
        assert_eq!(b.kind, BrushKind::Crease);
        assert!((b.radius - 0.123).abs() < 1e-5);
        assert!((b.strength - 0.77).abs() < 1e-5);
        assert_eq!(b.falloff, brush::Falloff::Sharp);
        assert_eq!(b.blend, BlendMode::Multiply);
        assert!((b.paint_color - Vec3::new(0.2, 0.4, 0.6)).length() < 1e-5);
        assert!(b.lock_plane);
        assert!(b.alpha_follow);
        // The stamp is found again by name, wherever it now sits.
        assert_eq!(b.alpha, Some(1));
    }

    /// A brush whose stamp is not loaded must still come back, without one.
    #[test]
    fn a_brush_survives_a_missing_alpha() {
        let mut brush = Brush::default();
        brush.alpha = Some(0);
        brush.strength = 0.31;
        let saved = vec![io::NamedBrush { name: "Stamper".into(), brush }];
        let path = std::env::temp_dir().join("sculpt_rs_missing_alpha.brushes");
        io::write_brushes(&saved, &["Scanned stone".to_string()], &path).unwrap();
        let back = io::read_brushes(&path, &[]).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].brush.alpha, None);
        assert!((back[0].brush.strength - 0.31).abs() < 1e-5);
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

        let far_right = s.mesh().pos.iter().map(|p| p.length()).fold(0.0f32, f32::max);
        let left_max = s
            .mesh()
            .pos
            .iter()
            .filter(|p| p.x < -0.5)
            .map(|p| p.length())
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
        let red = m.vertices().filter(|v| v.col.x > 0.9 && v.col.y < 0.1).count();
        assert_eq!(red, 25);
    }

    #[test]
    fn masked_vertices_resist_a_fill() {
        let mut m = primitives::cube(4);
        for v in 0..m.vert_count() as u32 {
            m.set_mask(v, 1.0);
        }
        let before = m.col(0);
        topology::fill(&mut m, 0, FillScope::Object, Vec3::X, BlendMode::Normal, 1.0, 35.0);
        assert_eq!(m.col(0), before);
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
